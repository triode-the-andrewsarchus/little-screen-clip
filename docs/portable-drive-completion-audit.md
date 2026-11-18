# Portable-drive completion audit

The accepted plan remains the completion criterion. The previous goal turn made progress (implementation commits and test evidence), but did not prove every requirement. This audit treats unverified or incomplete items as unfinished. No item is marked complete solely because a broader test suite is green.

Audit starting revision: `7202c4ec`. Personal `.cargo/config.toml` and `config.toml` edits remain outside the task. Existing evidence is recorded in `docs/portable-drives.md`; further tests must match each requirement's scope. Host gates do not excuse independent implementation work.

| Plan item | Audit status | Current evidence / remaining work |
| --- | --- | --- |
| 1.1 | Complete | Numeric parent/partition capacities, stable filesystem/PARTUUID evidence, read-only state and mount routing are covered by discovery tests and native authority fixtures. The new actual GPT reconnect fixture preserves enrollment across changed /dev/loopN partition paths, disk sequences and mount directories on FAT32/exFAT/NTFS; parent capacity matches the whole backing image while partition capacity matches the selected partition. See the native reconnect checkpoint. |
| 1.2 | Complete | Actual duplicate GPT partition IDs suspend automatic writes, retain queued local edits and resume after the clone is detached on FAT32/exFAT/NTFS; explicit sync refuses and the clone image hash stays unchanged. Missing PARTUUID now has explicit filesystem-UUID disclosure and enrollment in Pair/backup review, stable local records and fs-UUID buckets across restart. Native whole-volume sources and dark/light IPC-driven UI checks pass. Neither-UUID enrollment refuses without creating its target. See the filesystem fallback checkpoint. |
| 1.3 | Complete | Reloadable DriveConfig and guarded setting-preserving preference writes pass lazy selection, restart, raw-path, comment, symlink, permission and concurrent/replaced-file tests. Public invalid-field reloads preserve runtime/durable settings before destination creation. Native FAT32/exFAT/NTFS live-session tests cover all five reservation reductions, exact retained limits, future memory/capacity/directory admissions, continued writeback, catalog limits, backup destination/retention and restart from a saved configuration. Daemon IPC now names the asynchronous settings operation and reports admission refusal. See the live configuration checkpoint. |
| 1.4 | Partial | SQLite operations/baselines/conflicts and durable backup manifests now exist; ready-publication recovery is fault-tested. RAM preflight/allocation journals and atomic session/pair enrollment now recover unpublished setup without deleting published data. Ordinary sync installs/removals now have per-path receipts and separate confirmed counters, atomically committed with common baselines; cancellation, journal faults, killed-worker startup and native retry checks pass. Conflict-variant/export/backup item receipts and persistent operation coverage accounting remain. |
| 1.5 | Partial | Bounded admission and retained operations now bind client UUIDs and serialized request fingerprints atomically. Concurrent retries, changed-parameter refusal, queue/transaction failure, completed/restarted results and orphaned-owner recovery are tested; per-item interrupted-operation reconciliation remains. Full reconciliation, RAM data population and RAM export now use four retained worker slots, coalesced per-pair events and conflicting-drive/destination deferral. A 100 GB sparse initial sync and a separate 1 GB RAM population both leave another pair responsive. RAM metadata preflight, quota admission, formatting and initial mounting now use one retained preparation worker; image preflush and unmount authorization now use a retained partition worker. Policy observation now uses one retained worker with latest-input coalescing, bounded rule waiters, fail-closed publication and actual-exit shutdown ownership. Controlled and real stopped-FUSE tests keep configuration and another pair responsive. Routine discovery/RAM-availability refreshes now use a retained coalescing worker; stale settings results are rejected, policy obtains fresh mount-bound inventory and shutdown drains actual probe lifetimes. Missing Save-RAM scheduling destinations are planned without creating them. Four retained read-only scheduling probes now move root/selector observations off the coordinator, preserve slots through cancellation and drain before shutdown; saturation, real stopped-FUSE and simultaneous RAM-setup delivery checks pass. Enrolled reconciliation root preparation now shares the four retained sync slots, compares current enrollment/RAM/managed inputs before dispatch, and repeats managed-path checks in the write worker; held and real stopped-FUSE cases pass. Explicit RAM recovery/availability and export-destination preparation now share the retained RAM worker, preserving helpers/claims through cancellation and shutdown; held recovery and stopped export targets leave other pairs live. Watch registration/traversal now use four retained slots with per-root admission and coordinator-only event routing; stopped-source, independent-registration and actual-exit retirement checks pass. Configuration reload destination preparation now has an independent retained worker and transactional metadata/quota publication; stopped-target, cancellation, stale-input and legacy-root checks pass. Explicit preference destination/file saves and reviewed-backup continuations now retain worker completion, claims and saved-effect receipts; real stopped-config-filesystem cancellation/shutdown and superseded-reload fixtures pass. File-backup/catalog root preparation now shares scheduling workers, retains prepared roots and rejects stale settings/root inputs before copying; host discovery-thread and five real stopped-FUSE scenarios pass. Eject/image/read-only root and sync-plan preparation now retain one existing partition-worker slot, preserve original scheduled drive facts and compare current settings/enrollment/RAM metadata before flushing; six held and nine stopped-FUSE scenarios pass. Automatic/pre-unmount iteration now includes legacy pairs beyond status pagination. Initial discovery and RAM allocation/view startup recovery now reuse retained workers, with held inventory/enrollment/fsck, orphaned-enrollment, uncertain optional-policy and actual-exit shutdown checks. Catalog startup cleanup and mount/unmount now use a retained control worker, preserving actual helper ownership, atomic publication/retirement and failed boundaries; held commands, real stopped-FUSE, orphan startup and beyond-page recovery pass. Image mount/unmount now shares the retained backup pool, with local effect journals that pause optional indexing before an unknown UDisks mount location is admitted. Held-authority, cancellation, receipt-fault and provable restart cases pass; measurement admission, retention and the broader root/loop-lifetime responsiveness matrices remain. Safety validation of configured ordinary roots can still wait on those filesystems; independent-sync checks here use one-shot reviewed destinations. Large-export responsiveness and remaining mount-control scheduling need further evidence. Normal daemon signal handling now closes admission under the admission lock, cooperatively cancels queued/active work and joins the owner from a blocking context. Accepted-record completion and singleton release are tested. |
| 1.6 | Partial | Pinned roots and overlap policy project bind subtrees into observed filesystem coordinates. Descriptor-free scheduling scopes retain that observation after pins retire; they cannot authorize payload access. Watch/cache retirement covers overlapping observed regions. Pure projection, real FUSE unmount and startup RAM-backing refusal pass. A final-file bind-mount payload escape is fixed; lookup now uses kernel NO_XDEV before foreign metadata/open hooks, and the retained input still must match the root's device and mount ID. Guest tests prove same-device and cross-device leaf binds cannot supply hashes or backup contents on FAT32/exFAT/NTFS. Observed directory/file bind aliases now share core mount coordinates, with stale-context refusal and native FAT32/exFAT/NTFS policy tests. Case-fold aliases, the wider mutation-admission matrix, remaining managed-root/image coverage and full authorized-device lifetimes still need the complete matrix. |
| 2.1 | Complete | Guarded partition/subtree enrollment and persistent pair identities are tested. New bidirectional initial/replacement cases interrupt after the first durable file with cancellation, source disappearance, same-size/mtime source changes, destination changes or baseline-publication faults. Reopened registries retain only verified common baselines, preserve installed identities and recover without needless copies. Failed deletion-baseline commits cannot resurrect removed files. All 22 scenarios pass on host storage and real FAT32/exFAT/NTFS sources. See the common-baseline recovery checkpoint. |
| 2.2 | Partial | Native FAT32/exFAT/NTFS3 union and concurrent same-timestamp edits preserve both conflict variants; FAT32/exFAT case-fold collisions explicitly fail with both local originals retained. Real overflow now recovers both-sided union and same-size/mtime conflicts through the live daemon without a GUI or periodic scan. Clean eject and changed device/mount-path reconnection now preserve both offline edits and union on all three formats. Unsafe removal during accepted writes still needs its separate interruption evidence. |
| 2.3 | Partial | Watch overflow clears the root observation epoch; coarse/unknown filesystems never reuse full-scan fingerprints. Full-scan fingerprint reuse additionally requires complete watch registration, while named paths retain fresh bounded comparisons. Native lost-event same-size/mtime conflicts and watcher-unavailable recovery pass. Execution now also enforces per-root cache capabilities, and directory-change fallbacks use fresh scans instead of inheriting named-path coverage. Poisoned-cache controls verify both branches and native coarse-source refusal. Further reconnect matrices remain. |
| 2.4 | Partial | Persistent collision-safe complete conflict variants tested; provenance and repeat behavior need a full matrix. |
| 2.5 | Partial | Deletion and delete-versus-edit tests exist; event-loss and cancellation matrices remain. |
| 2.6 | Partial | Native file/directory name tables verify exact directory entries and bytes, or explicit refusal with originals retained. NTFS3 alternate-stream paths and FAT32/exFAT trailing dots/spaces are refused before installation. Native FAT32 oversized-file refusal passes; link/metadata omissions, alternate mount options and ntfs-3g remain. |
| 2.7 | Partial | Changed-file reconciliation, overflow invalidation and independent-drive scheduling tests pass. Coalesced path batches are shared and capped at 4096 paths/1 MiB; registration is incremental with 8192-watch/16 MiB/depth-256 budgets. Real repeated kernel overflow on FAT32/exFAT/NTFS rebuilds missed nested-directory watches; the live daemon recovers union/conflicts and later nested edits with a one-hour scan interval. Real watch-quota and instance-exhaustion recovery now retry at the periodic boundary, avoiding a continuous full-scan loop. Configured traversal-budget/permission failures and active-worker event-loss matrices remain. See the watcher retry and overflow checkpoints. |
| 3.1 | Partial | Capacity, MemAvailable and expanded contents are checked before admission; configurable per-session/aggregate byte, inode and session reservations now exist. Native quota and aggregate refusal tests pass; full pressure matrix remains. |
| 3.2 | Complete | Pinned filesystem and effective-user write/search/read-only checks select a usable tmpfs; RAM and backup admission share the directory check. A disposable guest covers /tmp with each native FAT32/exFAT/NTFS mount: all 36 selection cases pass, including XDG/shm fallback, read-only/unavailable candidates, explicit overrides and writable-tmpfs /tmp preference. Positive cases create 0700 private directories/0600 images on verified tmpfs, mount/write/read/discard successfully. Existing quota/restart/unmount tests pass. See the RAM-directory checkpoint. |
| 3.3 | Complete | Bounded streaming runs on retained workers; initial and explicit-resume jobs use catch-up before successful ready publication. Per-file callback cases deterministically change a same-size/same-mtime source and add an unenumerated path after the first installation, with a one-pass negative control. Cancellation, missing source and changed source preserve accepted files, incomplete/paused state and pending work; verified retry enables browsing. Both cases pass on real FAT32/exFAT/NTFS sources and bounded RAM destinations. Live coordinator cancel/resume and 1 GB independent-session responsiveness pass. See the initial-population checkpoint. |
| 3.4 | Complete | RAM sidebar shows connection state and bounded observed pending-path counts, with explicit full-check/unknown states. Menus and Jobs show the stable drive ID, RAM path and last confirmed drive sync separately from export history. Native removal/offline restart/reconnect and dark/light UI acceptance pass; acknowledgements preserve newer events and exports cannot advance drive confirmation. Counts remain advisory and never authorize eject/discard. See the RAM pending-status checkpoint. |
| 3.5 | Partial | Real bounded-media ENOSPC and controlled removal during a confirmed partial FUSE write pass. Failure/cancellation retain the original portable file, complete RAM edit, pending work and previous confirmation. Reconnect preserves both concurrent conflict variants and converges after catch-up. Native FAT32/exFAT/NTFS ENOSPC and kernel-read-only writeback now also preserve the old portable file, dirty RAM, pending state and prior confirmation, and recover after capacity/access returns. Controlled mid-write USB transport removal now passes on all three kernel filesystems, including retained RAM, further same-file offline edits/export, owner restart, actual replug and verified staging cleanup before union. Broader interrupted-I/O matrices remain. |
| 3.6 | Partial | Eject includes paused ready RAM mirrors and refuses incomplete population/failed preflush. Native tests also queue eject and unconfirmed discard behind a held write, preserve ownership through cancellation, keep unrelated controls responsive and retain terminal results after reconnect. The same paused-session preflush/queued-discard barrier now passes with native full and read-only FAT32/exFAT/NTFS sources. Remaining interrupted-write and eject/discard cancellation combinations still need evidence. |
| 3.7 | Partial | Native quota, restart, directory-fsync kill and cancellation fixtures pass. Bounded guest tests prove available-memory admission refusal, underlying-tmpfs exhaustion/recovery, acknowledged external writes retained across abrupt daemon death while held open, 13 namespace/metadata edits without later data writes/fsync, and immutable-flag recovery with correctly refused truncation. The helper propagates eviction/operation errors and flushes successful mutations before acknowledgement. Interrupted unacknowledged operations and broader concurrent pressure matrices remain. |
| 4.1 | Partial | Lazy selection produces a five-minute review bound to retained partition/destination roots. Cancel/expiry, replaced roots, changed partition facts, one-shot destination after preference changes and failed preference writes are backend-tested. Pending reviews are bounded and released before unmount. Dark/light focused UI runs prove cancellation, file backup publication, and image review; alias and broader failure matrices remain. |
| 4.2 | Partial | Durable checksummed SQLite manifests and filesystem/registry current-version references survive reference-installation faults; pruning removes matching manifests and protects current across clock changes. Admission/execution check working headroom; independent images require their full size estimate before unmount, while Btrfs permits shared allocation with explicit uncertainty. Detailed file-allocation estimation remains. |
| 4.3 | Partial | Bounded retries, metadata fidelity and hard-link preservation/reuse pass native checks. Read-only source guards now reject writable backing filesystems and policy loss on real FAT32/exFAT/NTFS, retaining current; ordinary live collection is preserved. Selectable reviewed/CLI read-only collection now owns flush/unmount/verified mount/copy/reader retirement/restoration. Real UDisks guest busy refusal, cancellation and late-cancel completion pass on all three source formats. Desktop password-agent, broader fidelity/mount-option and interruption matrices remain. |
| 4.4 | Partial | Real read-only Btrfs version snapshots and extent sharing passed; capability failure/admission matrix remains. |
| 4.5 | Partial | Real 4 KiB edits allocate 256 KiB, old bytes intact; strengthen final target verification evidence. |
| 4.6 | Partial | Explicit retention now protects open providers with cross-process directory leases and checks mounted catalogs beyond status pagination. Durable withdrawal/quarantine, paused recovery and unprivileged Btrfs fallback now pass targeted and native-image acceptance. Broader interruption and responsiveness matrices remain. |
| 5.1 | Partial | Preflush and unmount run as retained stages. Real owned FUSE integration proves external-reader busy refusal, successful pin retirement, independent RAM writeback during a held unmount and retained completed eject after cancellation. Image cancellation after unmount prevents copy admission. Admission retains the device-node pin and checks its disk sequence across authorized image reads. Reviewed requests show exact capacity/destination/free bytes and bind the selected roots; non-Btrfs full-image space failure precedes unmount. Real guest UDisks/PolicyKit authorization, busy-source refusal and successful preflush/unmount/image/restoration now pass. Interactive password-agent and hotplug/replacement races remain. |
| 5.2 | Partial | Isolated real UDisks/PolicyKit tests now prove authorized image reads, unprivileged denial and busy-device refusal on FAT32/exFAT/NTFS partitions. The desktop authentication-agent/password interaction remains a separate UI gate. |
| 5.3 | Complete | Exact authorized-device/image hashes, partition sizes, durable publication and checksummed source manifests pass on real FAT32/exFAT/NTFS GPT partitions. Manifests retain typed partition number/start/byte offset/logical sector size/type/label/flags and immediate containing-table type/identifier; unknown fields remain unknown. Native tests assert actual GPT layout and exact manifest retention. See the partition-layout checkpoint and its guest report. |
| 5.4 | Partial | Real Btrfs image delta and exact nonzero sectors pass; real non-Btrfs partition images now pass on FAT32/exFAT/NTFS with exact source SHA-256 and retained earlier images. Mount-option and interrupted-device matrices remain. |
| 5.5 | Partial | Optional mounted-state restoration now retains actual cleanup completion after imaging, failure or cancellation, refuses changed device lifetimes and reports separate restoration failures without losing completed backups. Owned authority and real FUSE fixtures pass; Real authorized image success and mounted-state restoration now pass on all three guest source formats. Final UI and interrupted-device acceptance remain. |
| 5.6 | Partial | Real UDisks loop setup/read-only mount/read/unmount passes for each native partition-image format; writes return EROFS. Real FUSE catalog reads through mounted images now pass, with ENOMEDIUM before/after provider availability and ESTALE for lost timestamp precision. Descriptor-bound cleanup now retries recorded partial unmounts, rejects a reused loop and preserves actual detach results on FAT32/exFAT/NTFS with ordinary and Btrfs destinations. Known-host previous-boot cleanup now also covers unrecorded setup, ready views and partial unmounts without touching reused loop numbers; see the real-reboot checkpoint. Current-boot unidentified outcomes, legacy contexts, exFAT FUSE precision compatibility and concurrent-provider matrices remain. |
| 6.1 | Complete | Version-2 paginated SQLite catalogs preserve raw names, numeric sizes/types, original modification/ctime fields, captured Drive and pinned source mount/namespace, explicit known/unknown driver precision, per-directory coverage and typed stage/reason/errno. Host permission/replacement/raw-name tests, indexed multi-page reads, real FUSE details/errors and native FAT32/exFAT/NTFS precision/metadata checks pass. See the typed catalog coverage checkpoint. |
| 6.2 | Partial | Independent catalog/search rules exist and full backup/sync inclusion is preserved. Exact read-only lookup now loads old whole-drive/folder settings independently of status pagination; dark/light UI acceptance verifies unchanged saves retain both flags. CLI writes require both flags, omitted subtrees work, and successful rule jobs publish exclusions before completion. Metadata observation now runs off the coordinator; fresh retries and stale-result rejection preserve that completion contract without blocking independent work. Explicit dated Jobs/CLI measurements now separate required sync baselines, inventories and pending bookkeeping from optional catalog files without touching source media or enabling indexing. Cancellation, real SQL deadline and offline persistence checks pass. Device selectors bind to persistent IDs before acceptance; lexical spelling aliases migrate transactionally with exclusions preserved. Queue-time device reuse, replay, clone refusal, offline editing, 1,100-rule migration rollback and dark/light CLI/editor checks pass. Observed bind aliases now use descriptor-free filesystem coordinates and mount-lifetime checks, including retained offline owners and first-time partial views. Filesystem case-fold/relocation aliases and legacy device-only policy review remain. |
| 6.3 | Partial | Search exclusion routing is reused by Shelf reference summaries, recursive photo discovery/features and audio constellation discovery/cache reads. Excluded roots are rejected before descent, with rechecks before decoding and publication. Isolated fixtures cover policy changes, cached results, symlink discovery aliases and explicit browsing/reads. Observed policy changes now retire Shelf summaries, photo similarity state and audio map discovery while retaining mutation owners. Shared policy matching now covers observed symlink/directory/file bind aliases, new mount events, renamed mount targets and retained offline observations; real FAT32/exFAT/NTFS gates pass. Broader search/answer/image-order caches, case-fold/relocation aliases and the remaining consumer inventory still need verification. |
| 6.4 | Partial | Future scans honor subtree rules. Saved descendant records now have durable cleanup intents, mounted-reader retention, compaction, restart recovery and separate capture/redaction dates. Real FUSE and dark/light Files acceptance cover pending cleanup, busy readers and reopened Not catalogued boundaries. New captures bind optional live name resolution to their original non-reused mount lifetime; native case aliases, case-only renames and bind-replacement refusal pass. Unresolvable aliases now retain their intent with an error. Arbitrary offline/legacy/remount aliases and concurrent-provider matrices remain with 6.3/6.8. |
| 6.5 | Complete | Configurable entry/database-byte/depth/time budgets, explicit larger-run review and saved effective limits pass host/native checks and dark/light acceptance. The million-file fixture respects exclusions and caps. Reloadable catalog_workers defaults to one and admits 1..4 independent captures on the retained bulk pool; live increase/decrease, four simultaneous captures, overlapping-source serialization, queued backup exclusivity and concurrent shutdown/restart pass on host and FAT32/exFAT/NTFS. Journaled cancellation/restart cleanup and sealed publication pass all ten lifecycle cases on each format. See the catalog worker and capture lifecycle checkpoints. |
| 6.6 | Complete | Indexed 512-entry pages preserve raw names, stable inodes and resumable offsets without directory-wide sorts. Real FUSE verifies correct nested dot/parent inodes, the maximum signed cookie, invalid-row EOVERFLOW without server poisoning, partial-listing EIO, completed-parent ENOENT, typed metadata, signed dates, invalid-date EOVERFLOW and multi-megabyte descriptor-backed reads. Original special-file types/link bytes/device numbers remain inspectable through bounded metadata while inert entries skip content jobs. Twenty-two valid mutation calls return EROFS; descriptor access-mode errors retain Linux EINVAL/EBADF. See the inode, type and timestamp checkpoints. Provider revision policy and the broader provider-lifetime matrix remain separately tracked in 6.7/6.8. |
| 6.7 | Partial | Provider metadata/digests validate the same retained descriptor; same-size/mtime changed bytes are rejected when a digest exists, and a stale newer backup falls back to an older match. File backups require their captured directory identity; metadata-only entries additionally require a verified matching-source manifest and retained-file checksum. Capture now reuses verified current/historical file-backup and enrolled portable sync hashes without source payload reads. Sync reuse binds full revision, subtree, source birth identity, boot/namespace/unique mount lifetime and row generation; legacy/mismatched rows require fresh hashing. Named portable observations refresh proven rows; incoming copies await observation. Native FAT32/exFAT/NTFS decline unsupported promotion and full-scan reuse. Historical candidate enrichment now uses independent transactions, bounded decoding, indexed registration order, a shared checksum allowance and early completion; explicit current pointers keep priority. Uncertain revisions and cross-driver precision handling remain. |
| 6.8 | Partial | A real separate-process FUSE fixture proves an existing handle returns ESTALE after its provider changes while a new open selects a newer valid backup. Replaced file-backup directories and catalog self-mounts are rejected. Lookup-time mount boundaries also reject a catalog file bind-mounted into a backup without recursively opening its server. Mounted images now require an atomically published local mount observation bound to boot/namespace, original image/loop, root identity and a non-reused kernel mount ID; same-root bind replacement, missing/changed provenance, held-file staleness and restoration pass on FAT32/exFAT/NTFS images. Root aliases and the broader concurrent-provider/interrupted-cleanup matrix remain. |
| 7.1 | Partial | Core guarded/source and drive tests pass; Old unbound operation records are protected, and atomic idempotent admission/restart/lost-owner fixtures pass. Ordinary-sync completed-item cancellation, receipt/baseline rollback, killed-worker startup and retry now pass on FAT32/exFAT/NTFS. Additive receipt-schema migration preserves supported versions 1–3 and legacy progress without inventing completion. Other mutation types, interrupted-intent reconciliation and migration cases remain. |
| 7.2 | Partial | Daemon/reconnect/restart fixtures pass. Real filesystem event loss now recovers union, conflicts, default restoration and later nested edits with no file-manager process. Mount-inventory loss and broader interruption state tests remain. |
| 7.3 | Partial | Commit `6b876b69` supplies a disposable KVM guest. Real FAT32/exFAT/NTFS format/write/unmount/remount probes pass without host mount privileges or attached media; failed-command and timeout probes are rejected. Eight application tests per filesystem now pass: union/conflicts, deletion policy, versioned file backups/manifests/timestamps, catalog exclusion versus full backup/sync inclusion, case/name handling, and applicable NTFS-stream/FAT32-size checks. The filesystem matrix uses whole-volume loop files with filesystem-UUID fallback; partitioned-device image/flush lifecycles also pass on FAT32/exFAT/NTFS. Native RAM writeback additionally covers full and kernel-read-only sources on all three formats. The owned USB fixture also verifies actual mid-write transport removal/replug, native I/O failure, offline RAM/export/restart and staging recovery on all three kernel drivers. Remaining mount-option and interruption cases remain. |
| 7.4 | Partial | Real FUSE, Btrfs, authorized partition-image/loop-provider and temporary read-only file-collection tests pass. Provider concurrency, interrupted-device and capability-failure matrices remain. |
| 7.5 | Partial | Dark/light private acceptance now includes RAM pending counts, drive confirmation, offline export and reconnect, alongside catalog/policy/backup review flows. Filesystem-UUID fallback pairing, lazy backup review, exact bucket browsing, reconnect and service restart now also pass in both appearances. Resource, broader coverage and interrupted-recovery UI flows remain. |
| 7.6 | Partial | Million-file, delta, sparse-active-backup, RAM byte/inode limits, bounded guest available-memory pressure, underlying-tmpfs exhaustion and acknowledged data/namespace/metadata edits across daemon death pass. A 10 TiB sparse RAM entry is rejected before hashing. Actual repeated kernel overflow and subsequent prompt nested edits pass on native FAT32/exFAT/NTFS. Active-worker loss, concurrent pressure/admission and unacknowledged interrupted writes remain. |
| 7.7 | Partial | Prior worktree passed workspace tests, format, clippy and documented architecture review; rerun affected and final gates after remaining changes. |

## Descriptor-bound image cleanup checkpoint

Image controls now persist a bounded local `image-loop` attachment alongside the
backup. It binds the original image/storage identity, UDisks object, pinned device
node's non-reused mount lifetime and namespace/boot, kernel disk sequence and
exact regular-file backing device/inode. UDisks `OpenDevice("r")` supplies the
owned descriptor; node class/identity are checked before admission. This metadata
never enters the removable filesystem or the image payload. Existing backup and
catalog formats remain unchanged.

Native mount and cleanup retain that descriptor through their actual effects.
Cleanup revalidates the attachment and uses `LOOP_CLR_FD` on the owned descriptor,
not a recycled object pathname. The shared device admission module owns the
existing `BLKGETDISKSEQ` primitive used by both imaging and loop validation. The
Linux UAPI layout is explicit and tested; a regular file cannot supply loop or
disk-sequence facts. Pure status checks refuse changed generations, backing
objects, offsets, size limits, block-backed images and writable mappings.
Implementation sources checked for this decision are the
[Linux 6.18 loop driver](https://github.com/torvalds/linux/blob/v6.18/drivers/block/loop.c)
and [UDisks OpenDevice API](https://storaged.org/doc/udisks2-api/latest/gdbus-org.freedesktop.UDisks2.Block.html#gdbus-method-org-freedesktop-UDisks2-Block.OpenDevice).

An already unmounted filesystem no longer prevents retrying a recorded partial
cleanup. Current namespace mount inventory determines whether a live view needs
validation/unmount first. An unbound device with an advanced generation proves
the old attachment ended, permitting local metadata retirement without a native
mutation. A rebound device is refused. Legacy ready records can be adopted only
through their current observed read-only filesystem, matching loop backing and
device number. Unknown or stale legacy records remain protected.

Other raw-device readers can defer kernel retirement. That outcome retains its
journal and no-index boundary instead of claiming completed cleanup. After the
reader closes, explicit Unmount image can finish the local retirement. Actual
native detach remains complete when the final registry transaction fails, with
a warning; a later retry reconciles its recorded attachment. Final retirement
atomically removes the attachment and mount observation and updates the backup.
Retention protects even an undecodable orphan attachment. The worker additionally
compares its exact admitted backup header before entering native authority.

Accepted UDisks authority calls now use zbus's default no-method-deadline behavior.
The previous explicit two-minute timer could discard a late loop-creation reply.
Local SDK inspection confirms that an absent timeout awaits the method result;
connection/service failures still retain uncertainty. This is not evidence for
the separate desktop password-agent acceptance gate.

The ordinary backend passes 261 tests, with 115 opt-in cases ignored, in
`/tmp/portable-drive-image-loop-backend-final-v3.log`. The changed-input trigger
replaces the artifact atomically with journal admission and proves zero authority
calls, untouched bytes and retained exclusions. Affected backend/Files/CLI/daemon
all-target Clippy and full runtime builds pass in
`/tmp/portable-drive-image-loop-clippy-final-v3.log` and
`/tmp/portable-drive-image-loop-build-v3.log`.

The isolated authority probe proves that an unprivileged child can retire
an explicitly inherited authorized read-only loop descriptor. A second reader
defers retirement until last close; unbound generation observation then succeeds.
The native service fixture covers legacy adoption, failed unmount recording,
failed final retirement recording, a held raw-device reader and a reused loop
number. Reuse leaves the foreign loop and its bytes intact; cleanup can finish
after that owned foreign fixture is detached. Every held case keeps configuration
and independent sync live, retains shutdown ownership and verifies image bytes.
Restoring the old unconditional mount-evidence gate fails each source format at
the intended retry in
`/tmp/portable-drive-image-loop-before-vm-partial/report.json`; this negative
control is not accepted as a passing run.

The broader pre-final-refactor lifecycle run passes 51 tests across real
FAT32/exFAT/NTFS in
`/tmp/portable-drive-image-loop-v2-vm-lifecycle/report.json`. The additional Btrfs
subvolume destination run passes nine test invocations (24 native stage/fault
scenarios across the three source formats) in
`/tmp/portable-drive-image-loop-v2-vm-btrfs/report.json`. Both exit successfully
without timing out. The VM runner's opt-in `--btrfs` includes matching guest
modules; `vm_images.sh --btrfs-destination` creates only a guest-owned loop image
and subvolume. Existing default guest fixtures retain their original setup.

After final native-stage flattening and the shared counter fixture cleanup,
the exact current source passes 12 native test invocations with ordinary backup
destinations and nine with the Btrfs subvolume destination in
`/tmp/portable-drive-image-loop-final-vm-native/report.json` and
`/tmp/portable-drive-image-loop-final-vm-btrfs/report.json`. Both reports confirm
successful exits, no timeout and passing results for every source format. These
final runs include all descriptor probes, retained/cancelled native stages,
partial/reused attachment cases and ordinary-destination image/provider checks.
Changed-source rustfmt, Python compilation, shell syntax and whitespace checks
also pass. No host raw device, privileged mount or system service is used.

The architecture check reports no new production hotspot, clone group or cycle.
Local growth is 685 NLOC, 26 net functions and 161 aggregate CCN; average/max CCN,
duplication and cycles remain 3.50/274, 6.94% and 144. The combined native scenario
driver remains test-only CCN 21 after separating fault preparation and recovery
resources; it exercises three effect stages and five interruption/reuse cases.
An initially duplicated counter fixture now shares one authority owner, and the
UAPI/probe share the production loop implementation. The tool still classifies
the larger native test as REGRESSION; the original-baseline gate remains open at
32 net clone groups and one net cycle. Reports:
`.fitness/portable-drives-image-loop-local-final.json` and
`.fitness/portable-drives-image-loop-branch-final.json`.

This checkpoint did not complete the plan. The following reboot checkpoint adds
known-host old-boot evidence. Unidentified current-boot loop setup, legacy partial
mappings without a live view, uncertain host/node contexts and the broader
mounted-provider lifetime matrix still need their original evidence.
Automatic recovery does not replay native mutations or silently delete an
unidentified mapping. Other incomplete rows above retain their original scope.

## Retained image-control checkpoint

Image mount/unmount now uses the existing exclusive backup-worker admission.
The coordinator installs optional indexing's literal-root exclusion before any
UDisks effect can begin. A bounded local journal binds the operation, original
image identity and checkpoint. Repeated configuration cannot bypass that barrier
while an older policy observer is held, and stale observations cannot relax it.
Unrelated configuration and enrolled reconciliation retain their own workers.
Shutdown retains the accepted task and singleton through actual worker exit.

Known loop setup is compensated on cancellation before mount admission. Ready
mount and retired-loop effects keep successful results even when auxiliary
checkpoint writes fail. Mount observation and backup publication remain atomic;
explicit unmount requires current read-only mount-lifetime evidence. Native
authorization/busy refusals prove no effect only at their documented boundaries.
Uncertain native replies retain the journal, optional-indexing barrier, backup
retention protection and recovery notice. Recovery clears only matching,
provable Prepared/NoEffect/Ready/Retired evidence and never replays a UDisks call.
Filesystem proof runs before the short SQLite comparison/removal transaction.

First image-control admission atomically advances the local registry to version
3. Saved catalog databases remain version 2. An explicitly retained old helper
refuses a real saved catalog through the version-3 registry before mounting;
the check uses only an owned empty target. Its first two fixture attempts were
invalid (invalid catalog ID, then no lease file) and supply no compatibility
evidence. The corrected final check passes in
`/tmp/portable-drive-image-controls-legacy-helper-final.log`.

The backend passes 258 tests with 112 opt-in cases ignored in
`/tmp/portable-drive-image-controls-backend-final.log`. Five ordinary admission
tests cover held authorization, independent sync, cancellation/actual-exit
shutdown, changed ownership, uncertain outcomes, repeated policy publication,
failed exclusion installation, interrupted pre-effect admission and receipt-fault
recovery. Removing the new repeated-barrier protection makes the focused test
fail at its intended assertion in
`/tmp/portable-drive-image-controls-policy-barrier-before.log`; restored code
passes. Retention also protects a version referenced by an undecodable journal
even when no loop object was saved.

The disposable LTS guest passes 45 lifecycle tests across FAT32/exFAT/NTFS in
`/tmp/portable-drive-image-controls-v2-vm-lifecycle/report.json`, with successful
exit and no timeout. Each format exercises real retained LoopOwned, Ready and
Retiring gates: independent configuration and sync complete, cancellation keeps
actual effects, and shutdown waits for the held native owner. Ready's failed
journal removal leaves the completed read-only mount protected; restart validates
and clears its receipt without replaying LoopSetup, then explicit unmount works.
Separate native cases inject Ready/Retired checkpoint-write faults, refuse an
actually busy image unmount, verify exact image bytes and exercise catalog
providers through mounted images. The first guest run fails the new fixture's
second stage because UDisks restored its source at a different path. It is not
accepted evidence. The corrected fixture selects current inventory for each
stage and resets the conventional guest mount between independent tests.

Affected backend/Files/CLI/daemon all-target Clippy and the full runtime build
pass in `/tmp/portable-drive-image-controls-clippy-final.log` and
`/tmp/portable-drive-image-controls-build.log`; the acceptance fixture rebuild
also succeeds. Changed-source rustfmt and diff whitespace checks pass. Privileged
partition and UDisks operations remain confined to the disposable guest.

The private Files light-theme acceptance flow passes all 51 observations in
`/tmp/portable-drive-image-controls-v1-ui-light/report.json`. The offline catalog
and conflict-review screenshots were inspected: the excluded folder remains
visibly Not catalogued, and the persistent conflict shows both preserved variants
alongside separate drive-confirmation and RAM-export status.

The broader source-filesystem regression also passes on FAT32/exFAT/NTFS in
`/tmp/portable-drive-image-controls-v1-vm-filesystems/report.json`: 225 main tests
plus nine owned-process probes, 234 raw passing counts, no failures or timeout,
and successful results for each format. It retains the existing catalog, startup,
RAM recovery, partition/bulk/preference/configuration preparation, watch/overflow,
managed-policy, sync preparation, population and native filesystem checks, and
adds the ordinary image-admission and uncertain-journal retention cases. These
ordinary image cases repeat coordinator policy/lifecycle checks; actual UDisks
effects are covered by the separate partitioned lifecycle guest above.

The combined native mount/retirement functions initially had CCN 28/21 and the
journal constructor CCN 25. Separating native effect stages from atomic journal
admission removes all new production hotspots. Native calls share the existing
connection policy and one UDisks proxy constructor; completion-warning handling
has one owner. Local comparison still reports REGRESSION: 1,203 NLOC, 34 net
functions, 200 aggregate CCN and three net clone groups. Average CCN changes from
3.49 to 3.50; maximum CCN and dependency cycles stay 274/144, duplication falls
from 6.95% to 6.94%. The six-scenario held-control test remains a test-only CCN-19
finding. Targeted clone inspection shows fixture invocations of the existing
image writer and short lifecycle assertions; no second image implementation was
added. Original-baseline review remains open at 32 net clone groups and one net
cycle. Reports: `.fitness/portable-drives-image-controls-local-final.json` and
`.fitness/portable-drives-image-controls-branch-final.json`.

This checkpoint does not complete the plan. Unknown LoopSetup outcomes, old or
partial loop mappings and loop-object reuse still need stronger loop-lifetime
recovery; uncertain cases remain protected rather than being replayed or deleted.
The broader provider, ordinary-root, measurement, retention and interruption
matrices retain their original scope in items 1.5/1.6/5.6/6.8 and the rows above.

## Retained catalog mount controls checkpoint

Catalog mountpoint creation, helper readiness, `fusermount3`, child retirement
and startup cleanup now use one retained catalog-control worker. The coordinator
keeps one accepted task until actual delivery, transfers any ready server before
finishing its operation and serializes controls with metadata maintenance. The
worker pool outlives transferred servers. Existing independent reconciliation
workers and controls remain usable. A mount/unmount that actually completes is
reported complete even if cancellation arrived after its effect began.

Ready helpers survive changed-header or mount-publication failures under explicit
ownership. Mount observation and the catalog header publish in one transaction,
after comparing the exact saved header. Explicit unmount can retire a ready
server when the failed transaction saved no mount path. Unmount validates the
observed catalog source/type and recorded mount ID, refuses foreign/changed
ownership and atomically retires the mount registration with the latest header.
Child kill/wait remains on the worker, including shutdown after a busy failure;
failed cleanup retains its mount boundary. Normal accepted controls enter the
running state and retain their durable result.

Startup uses bounded saved-key pages, skips at most 32 inactive records per pass
and never probes a saved target on the coordinator. Namespace inventory can find
an owned orphan at its expected target even when no mount path was published.
Unexpected legacy paths and unknown/changed mounts remain protected for explicit
repair. Malformed key-page errors stop that automatic pass without a repeated
no-progress loop. The registry fixture places a retained registration after
2,202 inactive catalogs and beyond the status window; cleanup retires both saved
observations while configuration remains usable.

Nine owned-process cases pass for held mount readiness, mount-time shutdown,
changed headers, publication faults, held unmount cancellation/shutdown, normal
startup cleanup, startup shutdown and an unpublished orphan. Settings and an
actual independent pair complete during the held commands. The publication cases
prove that the ready mount still exists after failure, then unmount and remount
successfully. The same gate fails against `f2a8f31c` on the configuration two-second
responsiveness assertion: `/tmp/portable-drive-catalog-controls-before.log`.
The positive native run is `/tmp/portable-drive-catalog-controls-native-v2-catalog.log`.
It also stops an actual owned FUSE mountpoint parent, observes the blocked catalog
worker syscall and proves controls, independent sync and retained cancellation/
shutdown ownership before resuming it. The shared RAM recovery fixture passes
again in `/tmp/portable-drive-catalog-controls-native-v2-ram.log` after extracting
the common bounded test-child wait into its existing owner.

The backend passes 252 tests with 110 opt-in fixtures ignored; affected all-target
Clippy and the runtime build pass in
`/tmp/portable-drive-catalog-controls-backend-final.log`,
`/tmp/portable-drive-catalog-controls-clippy.log` and
`/tmp/portable-drive-catalog-controls-build.log` (backend/Files/CLI binaries).
Daemon all-target Clippy and its full debug build also pass in
`/tmp/portable-drive-catalog-controls-daemon-clippy.log` and
`/tmp/portable-drive-catalog-controls-daemon-build.log`. The cold daemon build
completed in 20m40s; its retained process was polled through actual successful exit.
Native guest catalog cases use
the shared disposable-source fixture also used by retention, capturing real
FAT32/exFAT/NTFS source metadata while retaining the owned command gates.

Final acceptance passes 207 main cases plus nine owned-process probes (216 raw
passing counts) across FAT32/exFAT/NTFS in
`/tmp/portable-drive-catalog-controls-v1-vm-filesystems/report.json`. Each profile
includes the nine command cases, the actual stopped mountpoint and beyond-page
startup retirement alongside the existing RAM, partition, scheduling, watch,
configuration/preference, managed-view and event-loss checks. The separate image/
provider/maintenance run passes 42 cases in
`/tmp/portable-drive-catalog-controls-v1-vm-lifecycle/report.json`. Both guests exit
successfully without timing out. The complete private light UI flow passes all
51 observations in `/tmp/portable-drive-catalog-controls-v1-ui-light/report.json`;
offline-catalog and conflict-review screenshots were inspected. Changed-source
rustfmt and diff whitespace checks pass. Host native tests use only owned,
unprivileged FUSE mounts; privileged partition/image work stays in the guest.

The initial combined mount/unmount functions had CCN 24/21. Separating guarded
target creation, server readiness, atomic publication, native unmount and atomic
registry retirement removes both production hotspots without moving away checks
or adding another implementation. Startup `recover` falls from CCN 20 to 12.
The local comparison still reports REGRESSION: 671 NLOC, 27 net functions,
115 aggregate CCN and three net clone groups. Average/max CCN (3.49/274),
duplication (6.95%) and cycles (144) are unchanged. The nine-case native dispatcher
remains a test-only CCN-16 finding; short fixture assertions overlap other native
ownership tests. Source selection and subprocess waiting share their existing
fixture owners. No dependency or storage-format change is added. The branch-wide
gate remains open at 29 net clone groups and one net cycle above the original
baseline. Local report:
`.fitness/portable-drives-catalog-mounts-local-final.json`.

The following image-control checkpoint addresses retained mount controls and
unknown-location indexing coverage. Metadata-measurement admission, remaining
ordinary-root validation, loop-owner recovery and broader retention responsiveness
still need their original gates. Neither checkpoint completes item 1.5 or the full
plan.

## Retained RAM startup and initial discovery checkpoint

Initial discovery now uses the existing retained status worker and reports
`Checking mounted drives` until its first observation completes. RAM allocation
cleanup and saved-image recovery use the existing one-slot RAM preparation pool.
The coordinator visits saved keys in bounded pages, skips at most 32 legacy
non-image rows per pass and holds the selected drive while its view recovers.
No new helper process mechanism, disk format or source-media metadata is added.
The existing per-image identity/fsck/mount path is shared with explicit recovery.

Recovery adopts an actually started server before considering cancellation,
shutdown or stale inputs. Shutdown drains the pending worker, retires its adopted
server and leaves unvisited sessions durable for the next start. Recovered root
pins are handed directly to watch registration; the coordinator does not reopen
the recovered filesystem. Missing sync enrollment does not prevent recovering a
retained image for browsing, but no enrollment/writeback is invented. Uncertain
optional catalog metadata still fails closed for indexing and mutations without
preventing this view-only recovery.

Moving cleanup off startup exposed a protection interval: unpublished allocation
paths now participate in the existing private-backing checks until their journal
is retired. Cleanup validates the registry key and refuses any backing referenced
by a published RAM session, including another session ID or the same stored
directory/image identity. The native cleanup fixture verifies the published edit
survives both the cross-ID and original same-ID refusal.

Controlled startup tests keep settings and an independent union/sync usable while
initial inventory is held. A second gate releases inventory while fresh enrollment
preparation remains held. The first implementation invalidated that preparation
with the initial status epoch and failed with `Enrollment inputs changed`:
`/tmp/portable-drive-startup-initial-epoch-before.log`. The first observation now
establishes status without invalidating independently verified preparation; later
changed observations still invalidate caches, and preparation retains its own
source, mount-signature, metadata and managed-storage checks. Both that case and
shutdown while inventory is held pass. The original synchronous startup also
fails the pending-status assertion in
`/tmp/portable-drive-startup-discovery-before.log`.

The owned e2fsck gate now covers normal startup recovery, held shutdown, missing
enrollment and uncertain optional catalog metadata, alongside the six explicit
recovery/export cases. Controls remain usable; independent sync completes except
in the deliberately fail-closed policy case. After helper retirement each saved
image is reopened independently and its original bytes verified. The host native
probe passes in `/tmp/portable-drive-startup-ram-native-v1.log`; the separate native
allocation test passes in `/tmp/portable-drive-startup-allocation-native.log`.
The backend passes 251 tests with 107 opt-in fixtures ignored; affected all-target
Clippy and the runtime build pass in `/tmp/portable-drive-startup-backend-final.log`,
`/tmp/portable-drive-startup-clippy.log` and `/tmp/portable-drive-startup-build.log`.

The initial combined native run is retained as failed evidence:
`/tmp/portable-drive-startup-v1-vm-filesystems/report.json`. It exposed the enrollment
race above in the preference fixture on FAT32/NTFS; the new deterministic gate
reproduced it before the fix. Its successful subset is not counted as full
acceptance.

The final filesystem run passes 198 main cases plus six owned-process probes
(204 raw passing counts) across FAT32/exFAT/NTFS in
`/tmp/portable-drive-startup-v2-vm-filesystems/report.json`. It includes the startup
tests and unpublished-allocation cleanup on every format, all ten gated RAM
recovery scenarios, and the preceding partition, bulk, preference, configuration,
watch, managed-root, source-scope and event-loss gates. The image/provider and
maintenance run passes 42 cases across the same formats in
`/tmp/portable-drive-startup-v2-vm-lifecycle/report.json`.
The bounded-tmpfs guest passes five main RAM cases, including pressure refusal,
backing-store exhaustion/restart, abrupt death with an open unflushed file,
thirteen namespace/metadata edits and immutable-flag recovery with refused
truncation: `/tmp/portable-drive-startup-v2-vm-ram/report.json`.
All guests report success without timing out. The complete private light UI flow
passes 51 observations in `/tmp/portable-drive-startup-v2-ui-light/report.json`,
covering RAM writeback, sync enrollment, lazy backup/image review, exclusions,
offline catalog browsing and preserved-conflict review. Offline-catalog and
conflict-review screenshots were inspected. Changed-source rustfmt and diff
whitespace checks pass. No host mount/elevation or attached-media operation was used.

Against `pre-retained-startup-recovery`, architecture reports REGRESSION:
536 additional NLOC, twelve net functions, 84 aggregate CCN and three net clone
groups. Average CCN (3.49), maximum (274), duplication (6.95%) and cycle count
(144) are unchanged. The RAM recovery primitive and native fixture setup are
shared with explicit recovery; no second mount/fsck implementation was added.
Remaining reported similarities are direct production-constructor fixtures,
helper-transfer completion adapters and short native assertions. A generic
adapter would hide their distinct ownership/error contracts. Lizard adds no
local CCN-threshold finding; Valknut flags the existing explicit-access admission
and allocation-cleanup functions after their new recovery/alias safety gates.
Those checks enforce separate admission and published-data boundaries.
The full branch remains REGRESSION: 26 net clone groups and one net cycle above
the original baseline, with duplication down from 7.30% to 6.95% and average
CCN up from 3.44 to 3.49. Broader architecture cleanup remains open. Reports:
`.fitness/portable-drives-startup-recovery-local.json` and
`.fitness/portable-drives-startup-recovery-branch-final.json`.

Catalog startup mount cleanup still performs synchronous filesystem work.
Measurement, catalog/image mount controls, retention and configured ordinary-root
validation retain their remaining responsiveness gates. This checkpoint does not
complete item 1.5 or the original goal.

## Retained partition preparation checkpoint

Eject, image and read-only collection now prepare on the existing one-slot
`drive-partition` pool. Scheduling returns a fresh complete selected-drive
observation even for unmounted partitions; preparation refuses changes before
pinning rather than selecting a replacement. Fresh inventory, metadata-only raw
device pins, reviewed/source roots, destination creation/space checks and
pre-unmount sync-plan roots all leave the coordinator. The final post-flush device
check also runs on that worker. Existing unmount authorization, watch retirement,
raw-read authorization and compensation remain retained phases on the same pool.

Preparation captures guarded reconciliation inputs rather than duplicating sync
root policy. The source region and reviewed destination retain their scheduling
claims through actual exit. A preparing backup holds the existing exclusive
backup/maintenance admission. On delivery the owner checks the settings revision,
managed and enrolled-root geometry, selected RAM ownership, mount signature and
observation epoch before checking the complete prepared claims. Failure,
cancellation or shutdown starts neither flushing nor unmount. Created destination
directories are retained. A reviewed remember continuation retains the original
prepared device and root ownership through preference saving and capacity waits.
Shutdown drains preparation before watch/RAM helper retirement.

The old pre-unmount and automatic paths silently used only 1,000 saved pairs.
Both now use the existing retained paged registry visitor. A regression fixture
places 1,101 paused legacy records ahead of an older active pair in status order.
An independent older pair still syncs while preparation is held; local edits in
the selected older pair are flushed before the owned unmount. Paused normal pairs
remain skipped and incomplete RAM population still refuses mounted-source unmount.

The backend passes 249 tests (107 opt-in fixtures ignored) in
`/tmp/portable-drive-partition-prepare-backend-final.log`. Held preparation covers
success, cancellation/shutdown, settings changes, new enrollment, changed disk
sequence and the legacy-page case. Fresh inventory for eject/image/read-only
operations never runs on the coordinator. The same check fails on `982c55a8`
with two owner inventory calls:
`/tmp/portable-drive-partition-prepare-before.log`.
A separate injected-error fixture verifies a failed scheduling inventory stops
all three partition requests before destination creation, a partition journal,
unmount or payload work; partition requests now propagate that scheduling failure
rather than using the tolerance retained for lightweight metadata controls.
Nine real stopped-FUSE cases additionally pass for paired local roots during
eject and reviewed image/read-only destinations, with successful preparation,
cancellation/shutdown and stale settings for each. The syscall belongs to the
partition worker; controls and an actual independent sync complete while it is
stopped. Cancellation retains the operation/singleton until the server resumes,
then starts no unmount or payload work. Successful image preparation reaches the
owned authorization refusal; these fixtures never authorize host raw-device I/O.
The initial combined host/native run also exposed the automatic-pair cutoff;
final guest evidence below supersedes its intentionally retained failed log.

Affected all-target Clippy and the runtime build pass in
`/tmp/portable-drive-partition-prepare-clippy-final.log` and
`/tmp/portable-drive-partition-prepare-build.log`. Changed-source rustfmt and diff
checks pass.

The final native-filesystem guest passes 189 main cases plus six owned-process
probes (195 raw passing counts) across FAT32/exFAT/NTFS in
`/tmp/portable-drive-partition-prepare-v3-vm-filesystems/report.json`. The new
partition group runs all six held outcomes and nine stopped-FUSE scenarios on
each source format, alongside the previous scheduling, RAM, watch, backup,
catalog and event-loss checks. The rebuilt source also passes 42 partitioned
image/provider/maintenance cases in
`/tmp/portable-drive-partition-prepare-v2-vm-lifecycle/report.json`, including real
authorized image creation, read-only collection and mounted-state restoration.
Both guests exit successfully without timing out.
The full private light UI flow passes menus, RAM writeback, union enrollment,
lazy backup/image review, subtree exclusions, offline FUSE and preserved-conflict
review in `/tmp/portable-drive-partition-prepare-v2-ui-light/report.json`.
Offline-catalog and conflict-review screenshots were inspected.

The v2 filesystem run also disclosed a timing assumption in the existing
managed-view policy fixture: its total-observer counter included previously
accepted valid inputs after registry changes. The fixture now counts inputs
carrying uncertain metadata, while asserting valid observations actually run.
Temporarily removing the production metadata gate makes that corrected assertion
fail with two uncertain observations:
`/tmp/portable-drive-partition-policy-gate-control.log`. The production gate is
unchanged. The v2 failed report is retained; the v3 filesystem run passes with
this test-only correction. Image lifecycle and UI source are unchanged by it.

The local comparison against `pre-retained-partition-preparation` remains
REGRESSION for the necessary handoff phases and regression fixtures: +588 NLOC,
+20 functions and +99 total CCN. Drive-kind validation, filesystem pinning and
freshness checks have separate responsibilities; no new function exceeds the
threshold. The owner loop now uses one shared preparation-pending predicate.
Valknut separately flags the changed `flush_partition` entity. Lizard measures
it at CCN 13 with four parameters; it reuses the accepted Admission for guarded
reconciliation, durable result publication, whole-filesystem flush and the final
device check. This necessary lifetime boundary is retained, and the wider
architecture gate remains open.
Average/max CCN remain 3.49/274, duplication remains 6.95% and cycles remain 144.
Seven extra clone groups comprise a short admission-error pattern and fixture
observation/suspension/cleanup patterns; no payload algorithm is duplicated.
Report: `.fitness/portable-drives-partition-prepare-local.json`.
The original branch baseline remains REGRESSION, with 23 net clone groups and
cycles 143→144; the full branch architecture gate remains open. Report:
`.fitness/portable-drives-partition-prepare-branch.json`.

Items 1.3, 1.5 and 1.6 stay Partial. Startup recovery, measurement and image/catalog
mount controls, retention, configured ordinary-root safety stalls, broader
interruption/resource/provider acceptance and final workspace/architecture gates
remain open.

## Retained file-backup and catalog preparation checkpoint

File-backup and catalog requests now prepare their source and destination/state
roots on the existing four `drive-scope` workers. Scheduling retains the actual
source pin and optional destination pin, and the returned claims include the
prepared destination. Fresh inventory, review pin validation, guarded destination
creation, managed-storage checks, space admission and final source/mount checks
complete off the coordinator. No extra pool, unbounded queue or second root-opening
path is introduced. Payload work keeps the existing exclusive-backup/configurable
catalog admission and worker-side guarded checks.

The shared configuration input snapshot now supports capture without starting a
settings change. Bulk scheduling retains that snapshot through actual worker exit
and compares current settings/revision, all enrolled-root geometry, managed roots
and mount observations before dispatching. A changed input fails with a retry
message and retains any created directories. Cancellation/shutdown retains the
blocked preparation before discarding its pins and starting no payload work.
The coordinator's early reviewed-backup check is expiry-only. Workers still
validate the original reviewed drive and pinned roots; partition preparation
retains its existing validation while its separate handoff remains unfinished.
The common destination routine now serves both ordinary and partition backups,
replacing the two owner-bound destination methods.

The payload worker repeats the managed-destination guard immediately before
backup recovery, space admission or payload creation. A regression test first
validates the destination, then enrolls it as a paired local folder and confirms
the already prepared worker refuses it without creating payload files or a
completed backup. This retains safety across the asynchronous handoff.

The backend passes 246 tests (106 explicit fixtures ignored) in
`/tmp/portable-drive-bulk-backend-final.log`. Host backup/review/catalog cases assert
zero coordinator inventory calls. On `2a89c373` the same test fails with two owner
calls in the first case: `/tmp/portable-drive-bulk-before.log`.
Two targeted tests, including five stopped-FUSE scenarios, pass in
`/tmp/portable-drive-bulk-native-v2.log`. They cover direct-destination cancellation,
reviewed-source success, reviewed-source cancellation/shutdown, stale configuration
and catalog-source success. The blocked syscall belongs to `drive-scope`; controls
finish while it waits. All reviewed-source/catalog cases additionally confirm an
independent sync. Cancelled/stale jobs publish no backup or payload. A direct FUSE
destination uses cancellation before copying: successful publication on arbitrary
FUSE destinations without a persistent identity is not proved by this fixture.

Affected all-target Clippy and the runtime build pass in
`/tmp/portable-drive-bulk-clippy-final.log` and
`/tmp/portable-drive-bulk-build.log`. The updated native-filesystem matrix passes
177 main cases plus six owned-process probes (183 raw passing counts) across
FAT32/exFAT/NTFS in `/tmp/portable-drive-bulk-vm-filesystems/report.json`, including
all three new cases on each format and the existing preference, configuration,
watcher, scope, enrollment, RAM, native backup/name and event-loss checks.
The rebuilt source also passes 42 partitioned image/provider/maintenance cases in
`/tmp/portable-drive-bulk-vm-lifecycle/report.json`. Both guests exit successfully
without timing out.
The private light UI acceptance flow passes drive menus, RAM writeback, union
enrollment, lazy file backups, image review, subtree exclusions, offline FUSE and
preserved-conflict review in `/tmp/portable-drive-bulk-ui-light/report.json`.
Offline-catalog and conflict-review screenshots were inspected. Changed-source
rustfmt and diff checks pass.

The local comparison against `pre-retained-bulk-preparation` remains REGRESSION:
+319 NLOC, +7 functions and +47 total CCN. Destination preparation, source
observation and post-delivery fixture checks have distinct responsibilities; no
new function exceeds the threshold. Average/max CCN remain 3.49/274, duplication
remains 6.95% and cycles remain 144. The two extra six-line clones are fixture
observer and suspension/cleanup setup, not duplicate payload algorithms. Shared
error-handling clones flagged in unchanged paths are pre-existing. Report:
`.fitness/portable-drives-bulk-local.json`. The original branch architecture gate
remains open: 16 net clone groups and cycles 143→144 remain in
`.fitness/portable-drives-bulk-branch.json`.

At that checkpoint partition preparation was still open; the later retained
partition checkpoint above covers its handoff. Items 1.3, 1.5 and 1.6 stay Partial.
Startup recovery, measurement/mount preparation, retention, broader interruption/
resource/provider acceptance and final workspace/architecture gates remain open.
Configured slow
ordinary backup roots can still delay independent synchronization's managed-root
safety checks; this checkpoint does not remove those validations.

## Retained backup preference checkpoint

Explicit `BackupDirectory` requests and reviewed backups with “remember” now
prepare their destination and save preferences on one bounded `drive-preference`
worker. The worker reuses the guarded core configuration writer; it preserves
comments, unrelated settings and revision checks. Preparation keeps the target
pinned and checks current storage, captured root geometry and mount observations
before saving. A shared settings revision rejects older preparation after a new
choice. The preference worker is independent of configuration reload preparation,
so an older reload stalled on another filesystem cannot prevent a newer choice.
Reload publication waits while an accepted preference save is committing.

Accepted saves retain the worker until actual exit. Cancellation is checked
before writing; once a file save begins, its actual outcome is delivered through
cancellation and shutdown. A known successful file save becomes the runtime
selection even if subsequent SQLite publication fails, with both outcomes in the
error. A file-writer failure reports uncertain installation and keeps the previous
runtime selection; a durability error may occur after replacement. No stronger
file/registry atomicity is claimed.

A successful settings transaction also stores a small local SQLite `preference`
receipt keyed by operation ID. Reviewed-backup completion, failure, cancellation
and interruption messages disclose that the setting was saved. Removable media
receive no service metadata. A crash between the file save and receipt publication
still relies on loading the authoritative configuration file and the generic
interrupted-effects message; per-item crash consistency remains unfinished.

Reviewed bulk and partition continuations retain their original roots/device
source and scheduling claims through preference saving and subsequent capacity
waits. Fresh review validation precedes copying or partition preparation.
Cancellation after saving finishes the backup without starting payload work;
later image-authorization refusal retains the saved preference. Successful file
backup, image refusal, registry-publication faults, restart/replay and malformed
preference tests cover the handoff. Existing preparation outside this preference
handoff still requires the remaining responsiveness work.

The backend passes 244 tests (105 explicit fixtures ignored) in
`/tmp/portable-drive-preference-backend-final.log`. Seven targeted cases including
real stopped-FUSE configuration-file saves, independent synchronization,
cancellation/shutdown and four concurrent reload outcomes pass in
`/tmp/portable-drive-preference-targeted-final.log`. Both explicit and remembered
saves wait on `drive-preference`, then report the installed preference after
cancellation. The remembered continuation copies no payload. Against `f887817d`,
the same stopped-file control times out after 15 seconds:
`/tmp/portable-drive-preference-before.log`.

Affected all-target Clippy and the runtime build pass in
`/tmp/portable-drive-preference-clippy-final.log` and
`/tmp/portable-drive-preference-build.log`. The updated native-filesystem matrix
passes 168 main cases plus six owned-process probes (174 raw passing counts)
across FAT32/exFAT/NTFS in
`/tmp/portable-drive-preference-vm-filesystems/report.json`. This includes all
three new preference cases on each format and reruns configuration, watcher,
scope, enrollment, RAM, native backup/name and event-loss checks. The guest exits
successfully without timing out.
The rebuilt source also passes 42 partitioned image/provider/maintenance cases
in `/tmp/portable-drive-preference-vm-lifecycle/report.json`, including authorized
image success, read-only collection, mounted-state restoration and real catalog
reads through images. That guest also exits successfully without timing out.
The full private light UI flow passes drive menus, RAM writeback, union
enrollment, lazy file backup, image review, subtree exclusions, offline catalog
reads and preserved conflicts in
`/tmp/portable-drive-preference-ui-light/report.json`. Reconnected-catalog and
conflict-review screenshots were inspected. Changed-source rustfmt and diff checks
pass. These are checkpoint checks; the final workspace gates remain open.

The local comparison against `pre-retained-drive-preferences` remains REGRESSION:
+694 NLOC, +23 functions and +111 total CCN. Destination selection and validation
are distinct phases, and partition flushing is separated from owner admission;
no new function exceeds the threshold. The prior partition-admission hotspot
falls from 22 to 8. Average/max CCN remain 3.49/274. Duplication falls 6.96%→6.95%,
with two additional short clones: shared inputs-error handling in the two settings
admission paths and a fixture-service constructor. They contain no duplicate
payload algorithm; adding a generic wrapper solely for these small call sites
would obscure ownership. Cycles remain 144; reordered pre-existing cycle members
are not new dependencies. Report:
`.fitness/portable-drives-preference-local.json`.

Items 1.3, 1.5 and 1.6 stay Partial. Startup recovery, remaining bulk/partition and
mount preparation, retention, broader interruption/resource acceptance and final
workspace/architecture gates remain open.

## Retained configuration reload checkpoint

`Configure` now prepares its backup destination on one retained `drive-settings`
worker, independent of scheduling probes, RAM setup/access, policy and status
workers. Directory creation, managed-storage validation and final root checking
leave the coordinator free to process controls and unrelated reconciliation.
The target pin survives until publication. A pending reload defers new RAM
allocation; allocation already in progress still defers reload admission, so an
unpublished reservation cannot escape a quota change. Existing RAM writeback and
independent jobs continue. The existing bounded deferred queue is reused.

Delivery checks cancellation/shutdown, the previous configuration, captured root
metadata and the target's mount signature. Final quota validation, metadata
comparison and SQLite settings publication share one transaction; the in-memory
configuration changes only after commit. Failed publication preserves the old
settings and retains any created directories for retry. Cancellation retains
the accepted operation and worker until actual exit. A concurrently completed
explicit preference change wins over the older prepared reload; a new enrollment
also invalidates a prepared destination. Per-pair status/last-success updates do
not invalidate unchanged root geometry, verified by a real independent sync
while configuration preparation is held.

Reloads without a backup destination need only configuration/quota validation.
They do not depend on optional catalog/view metadata or mount observation. A
missing catalog header therefore cannot prevent a basic reload used to repair
settings; optional indexing keeps its existing fail-closed policy. Selecting a
destination still requires the full managed-root/mount checks.

The existing retained, paged RAM-record visitor now also captures sync-pair
records. Configuration snapshots, backup destination guards and enrollment root
comparison cover every saved root, independently of the status-page limit.
The shared enrollment overlap routine now protects backup destinations too,
including missing-root lexical fences and observed managed-FUSE aliases. A
1,101-enrollment regression test protects the oldest folder, then removes that
owned folder and verifies its parent still cannot become a backup destination.
No public protocol, schema or persisted path representation changes.

The real stopped-FUSE fixture checks successful reload, cancellation/shutdown,
a concurrent explicit preference and concurrent enrollment. Settings preparation
waits on `drive-settings`; controls and a confirmed independent sync complete
while the destination remains stopped. A RAM admission queued behind the reload
can still be cancelled without entering that filesystem. The same fixture on
`cb7c8c9c` times out after 15 seconds waiting for a control request while the
coordinator is blocked: `/tmp/portable-drive-settings-before.log`. The updated
four-scenario fixture, uncertain-view/basic reload, transaction-fault retry and
legacy-root tests pass in `/tmp/portable-drive-settings-targeted-final.log`.

Backend validation passes 242 tests (104 explicit fixtures ignored) in
`/tmp/portable-drive-settings-backend-final.log`. Affected all-target Clippy and
the runtime build pass in `/tmp/portable-drive-settings-clippy-final.log` and
`/tmp/portable-drive-settings-build.log`. The native-filesystem matrix passes
159 main cases plus six owned-process probes (165 raw passing counts) across
FAT32/exFAT/NTFS in
`/tmp/portable-drive-settings-vm-filesystems/report.json`; configuration cases use
owned stopped-FUSE destinations and fixture pairs. This also reruns watch,
scope, enrollment, RAM, native sync/backup/name and event-loss checks. The only
subsequent source change collapses an equivalent conditional for Clippy. The
rebuilt source additionally passes 42 partitioned-drive lifecycle/provider/
maintenance cases in `/tmp/portable-drive-settings-vm-lifecycle/report.json`.
Both guests exit successfully without timing out.
The full private light acceptance flow passes drive menus, RAM writeback, union
enrollment, lazy file backups, image review, subtree exclusions, offline catalog
browsing and preserved conflicts in
`/tmp/portable-drive-settings-ui-light/report.json`. Offline-catalog and conflict
review screenshots were inspected. Changed-source rustfmt and diff checks pass.

The local `pre-drive-settings-preparation` comparison remains REGRESSION for the
necessary preparation/publication phases and fixtures: +451 NLOC, +16 functions
and +61 total CCN. Snapshot validation and transactional publication have distinct
methods, eliminating the draft's new hotspot. No new function exceeds the
threshold; the existing owner loop increases 26→27 for pending-settings polling.
Average/max CCN stay 3.49/274, duplication stays 6.96%, clone groups stay 4,301 and
cycles stay 144. The original branch baseline remains REGRESSION, including
twelve net clone groups and cycles 143→144. Reports:
`.fitness/portable-drives-settings-local.json` and
`.fitness/portable-drives-settings-branch.json`.

This checkpoint finished filesystem preparation for explicit configuration
reloads. Explicit backup-directory saves and the reviewed-backup “remember”
handoff were still on the coordinator then; the later retained backup preference
checkpoint above covers that work. Startup recovery, remaining bulk/partition/
mount preparation, broader interruption/resource acceptance and final workspace/
architecture gates remain open. Items 1.3, 1.5 and 1.6 stay Partial.

## Retained watch registration and traversal checkpoint

Watch registration, directory enumeration, file-type observation and root/frame
validation now execute on four retained `drive-watch` workers. A directory reader
is created, traversed and closed on its worker; no reader/continuation is returned
to the coordinator. Registration acknowledges its descriptor before traversal
finishes, so existing directory events remain routable while a large directory
is scanned. One enrolled tree occupies at most one slot, leaving independent
roots eligible. Pending registrations receive the existing 20 ms preparation
poll cadence rather than an additional idle interval per directory.

The coordinator drains the shared nonblocking inotify descriptor and treats names
as hints. It does not reopen/validate the content root when processing an event.
Registration and reconciliation repeat their guarded checks on workers. A kernel
descriptor can represent several bind aliases of the same inode; its location
list emits each observed spelling, and retiring one spelling preserves the
remaining registration. Create/move/delete directory events retire obsolete
subtree observations before replacement admission. A moved directory and a new
directory under its old name both retain their nested notifications.

Registration serials and instance epochs reject results after retirement/rebuild.
A late unclaimed current-instance acknowledgement invalidates coverage rather
than claiming a possibly reused descriptor. Root watches waiting for admission
survive overflow rebuild, while cancelled roots cannot be resurrected by old
acknowledgements/inventories. Rebuild replaces the current instance; up to four
retired worker-owned descriptor instances can remain until those workers exit.
The existing 8,192-address / 16 MiB / depth-256 admission limits and 4,096-path /
1 MiB event batches remain. Each of four directory inventories additionally has
an 8,192-child / 16 MiB result bound; retained registration-address storage and
temporary worker results are separate bounds. Incomplete/quota-limited coverage
keeps the periodic retry/fresh-scan fallback. Named hints retain fresh comparisons,
and full-scan fingerprint reuse is disabled through pending/incomplete coverage,
including a recheck after new watches are queued for a prepared sync plan.

Scope retirement cancels matching work but retains its pins through actual exit.
Partition preparation uses its already captured source scope, retires aliases,
and waits in a metadata-only retirement stage before admitting unmount. It no
longer reopens the source just to construct this retirement scope. Shutdown drains
watch ownership before partition unmount and RAM-helper retirement. These changes
reuse the existing task wrapper, scope projection and directory-frame identity
validator; the public protocol and persisted records are unchanged.

Two owned stopped-FUSE fixtures verify settings and independent writeback, event
draining, new independent-root registration, cancelled pin retention and shutdown
waiting for actual exit. The real daemon reports its blocked syscall on
`drive-watch`, not the coordinator. Against `fdccf90d`, the same directory-event
fixture instead observes the coordinator waiting on FUSE and a settings operation
times out after 15 seconds: `/tmp/portable-drive-watch-before.log`. The updated
fixtures pass in `/tmp/portable-drive-watch-stopped-final.log`.

Backend validation passes 239 tests (103 explicit fixtures ignored) in
`/tmp/portable-drive-watch-backend-final.log`. Affected all-target Clippy and the
runtime build pass in `/tmp/portable-drive-watch-clippy-final.log` and
`/tmp/portable-drive-watch-build.log`. The partitioned-drive run passes 42
lifecycle/provider/maintenance cases across FAT32/exFAT/NTFS, including real flush,
unmount/remount, authorized image backup and read-only image mounting, in
`/tmp/portable-drive-watch-vm-lifecycle/report.json`. That run uses the same
production source before the final shared-descriptor test-only addition.
The final native run passes 147 main cases plus six owned-process probes
(153 raw passing counts) in
`/tmp/portable-drive-watch-vm-filesystems-final/report.json`. It includes actual
queue overflow, quota recovery, shared-descriptor bind aliases, stopped watch
registration, directory replacement, managed-view aliases, RAM preparation and
native sync/backup/name checks on FAT32/exFAT/NTFS. Both guests finish successfully
without timing out. The private light RAM flow passes copy, pending changes,
removal, offline export and reconnect in
`/tmp/portable-drive-watch-ui-light/report.json`; offline and reconnected status
screenshots were inspected. Changed-source formatting and diff checks pass.

The `pre-retained-drive-watches` comparison remains REGRESSION for the necessary
retained registration/retirement mechanism and its fixtures: +439 NLOC, +22
functions and +72 total CCN. Splitting syscall registration from enumeration and
sharing fixture thread discovery/teardown removes the draft's new function
hotspot and clone group. No new function exceeds the complexity threshold; the
existing owner loop increases 25→26 for the pending-watch poll condition. That
condition supplies prompt registration delivery after the filesystem work moves
off the loop. Duplication stays 6.96%, average/max CCN stay 3.49/274, clone groups
stay 4,301 and cycles stay 144. Existing script/vendor cycle member-order changes
do not represent changed source dependencies. The original branch baseline still
reports REGRESSION, including twelve net clone groups and cycles 143→144.
Reports: `.fitness/portable-drives-watch-local.json` and
`.fitness/portable-drives-watch-branch.json`; the final architecture gate remains
open.

The complete responsiveness requirement remains Partial: startup recovery,
configuration/destination checks and remaining bulk/partition/mount controls
still require their handoffs and full matrix. Saturated/pressure and broader
filesystem/device-loss cases and final workspace/architecture gates remain open.
Items 1.5, 2.7 and 7.6 stay Partial.

## Retained RAM access preparation checkpoint

Explicit Synchronize/Flush RAM and Save RAM requests now prepare RAM access on
the existing single retained `drive-ram-setup` worker. Recovery, filesystem
checking, source availability, destination creation and permissions no longer
run in their coordinator request handler. New RAM allocations and recovery share
that worker; no additional helper-creator pool or public/persisted fields were
introduced. Ordinary reconciliation still uses its four preparation/copy slots.

The pending access retains the accepted envelope, cancellation token, partition
and scheduling scopes through actual delivery. Configurations and independent
pairs remain serviceable. Delivery checks the captured enrollment, authority
fields of the RAM session, managed storage and post-recovery mount signature.
Advisory session status/pending/save counters do not authorize access. A newly
recovered helper is adopted before interpreting a later preparation failure,
cancellation or shutdown; its creating worker remains alive until that helper
exits. Shutdown closes admission, drains RAM preparation and then retires sync
jobs/helpers. It does not discard the RAM image on a failed/cancelled export.

The output destination retains its scheduling observation as the last claim,
and preparation repeats that observation before creation. Missing-parent
replacement during recovery is refused. Output children are opened through the
retained base root's existing subtree operation. Preparation and copying share
one root/destination validator, repeating the private-backing, mounted-view and
source-overlap fences before source inventory. Successful export retains the
existing save-only confirmation semantics; failures/cancellation do not advance
the last saved or drive-flushed confirmation.

An owned subprocess fixture interposes only its own `e2fsck` command using a
child-specific PATH. Its held recovery checks settings, another pair's live
writeback and actual-exit cancellation/shutdown ownership. Six scenarios cover
successful offline export, cancellation/shutdown, changed managed storage,
replaced destination ancestry, recovery followed by drive flush and a stopped
FUSE export destination. The last observes `drive-ram-setup` waiting on a real
FUSE request while settings and another pair remain live; cancellation creates
no output tree. Failed/cancelled operations retain the original image contents.
No host settings, services, mount privileges or global process environment are
changed. Run the outer test exactly; `ram_access_owner_probe` is its internal
child, not an independent acceptance case.

The same fixture against `0a65a52d` fails after 15 seconds waiting for a settings
operation while recovery occupies the coordinator:
`/tmp/portable-drive-ram-access-before.log`. The updated six-scenario fixture
passes in about five seconds:
`/tmp/portable-drive-ram-access-recovery-final.log`. Backend validation passes
238 tests (100 explicit fixtures ignored) in
`/tmp/portable-drive-ram-access-backend-final.log`; affected all-target Clippy and
the runtime build pass in `/tmp/portable-drive-ram-access-clippy-final.log` and
`/tmp/portable-drive-ram-access-build.log`.
The disposable native-filesystem run passes 117 main cases and six owned-child
probes (123 raw passing counts) across FAT32/exFAT/NTFS in
`/tmp/portable-drive-ram-access-vm-filesystems/report.json`. This includes the
six-scenario RAM fixture once per format, managed RAM/catalog alias and stopped
server checks, scheduling/root preparation, population, sync/backup/names and
real event-loss recovery. The guest exits successfully without timing out.
The private light RAM flow passes copy, pending changes, removal, offline export
and reconnect in `/tmp/portable-drive-ram-access-ui-light/report.json`. Exported
offline and reconnected status screenshots were inspected: export leaves the
pending drive changes/old confirmation visible; reconnect reaches zero pending
changes with a new drive confirmation. Changed-source rustfmt and diff checks
also pass.

Local architecture baseline `pre-ram-access-worker` remains REGRESSION for the
necessary preparation/ownership checks and fixtures: 552 NLOC, twelve functions
and 92 total CCN added. Recovery and destination preparation have distinct
lifetimes, so their shared outcome retains the helper on partial failure;
existing task delivery, RAM mount/recovery, scope and root mechanisms are reused.
Consolidating export validation reduces `copy` CCN from 17 to 15. No new function
hotspot or clone group remains. Duplication changes 6.97%→6.96%, average/max CCN
remain 3.49/274, clone groups remain 4,301 and cycles remain 144. Cycle reports
that reorder members of unchanged script/vendor cycles are not source changes.
Reports: `.fitness/portable-drives-ram-access-local.json` and
`.fitness/portable-drives-ram-access-branch.json`. The original branch baseline
remains REGRESSION (average CCN 3.44→3.49, twelve net clone groups, cycles
143→144); that final gate is still open.

This completes the explicit RAM request-preparation handoff, not all daemon
filesystem work. Startup recovery, watch registration/traversal, configuration
and other mount/bulk control preparation still require their full responsiveness
matrix. Wider root/mount lifetimes, interruption/recovery acceptance and final
workspace/architecture gates remain open. Items 1.5/1.6 stay Partial.

## Registered RAM mount exclusions checkpoint

Registered RAM views now share catalog views' deny-only mount-table exclusions.
The policy snapshot captures every RAM record through the existing transactional
paged visitor, independently of the 1,000-row status display. A 1,101-record test
proves an older registration remains available to mount classification. Ordinary
unregistered FUSE data sources retain their existing treatment; the mount label
and a registered RAM ID establish an exclusion, never content-access authority.
Publication carries those IDs in its private input and excludes each observed
RAM bind target literally, with the same mount signature as the other rules.
It does not request attributes from the RAM server to publish the exclusion.

Synchronization, enrollment-target, backup-destination and RAM-export guards use
the same observed managed mounts. Their namespace paths cover bind aliases,
including a registered RAM alias remaining after its original mount is removed.
An own-session local RAM root still requires the existing access/identity checks
and remains usable. Other managed boundaries nested in that RAM filesystem keep
their fence: because resolving such paths could itself invoke a stopped server,
the observed managed FUSE superblock and its aliases are treated as an opaque
boundary for that check. This does not give an own-session exception permission
to enter another recorded backup/catalog/session region. Private backing paths
retain their existing checks. Orphan/missing-registration and wider image/root
lifetime cases remain with item 1.6.

The negative control `/tmp/portable-drive-ram-managed-before.log` times out after
15 seconds waiting for a rule to publish while its registered RAM server is
stopped. The corrected owned-server fixture completes in about one second,
publishes a verified policy without the global root exclusion, and synchronizes
an unrelated pair while the server stays stopped. It also checks unregistered
FUSE treatment, own-session access, managed-source/export-target refusal and a
nested foreign boundary. The disposable bind test verifies RAM alias exclusion
and mutation refusal after original-mount removal. Shared policy assertions
retain exact alias exclusion and verified-publication checks while removing the
draft's three new test clones.

Evidence: 238 backend tests pass (98 explicit fixtures ignored) in
`/tmp/portable-drive-ram-managed-backend-final.log`; the stopped-server fixture
passes in `/tmp/portable-drive-ram-managed-stopped.log`. The native run passes
114 main cases across FAT32/exFAT/NTFS, plus three owned-daemon probes (117 raw
test counts), in `/tmp/portable-drive-ram-managed-vm-filesystems/report.json`.
It covers both RAM/catalog origin-loss aliases, stopped registered RAM and
unrelated sync, enrollment/reconciliation admission, RAM population, native
sync/backup/name handling and actual event-loss recovery. The private light RAM
flow passes copy, pending changes, removal, offline export and reconnect in
`/tmp/portable-drive-ram-managed-ui-light/report.json`; its reconnected status
screenshot was inspected. Affected-package all-target Clippy, runtime build,
changed-source rustfmt and diff checks pass.

The local baseline `pre-ram-export-preparation` was captured before discovering
this prerequisite. Its comparison remains REGRESSION for added total CCN, with
216 NLOC, six functions and thirty CCN added. There is no new reported hotspot,
clone group or dependency cycle. Existing RAM decoding, bounded visitation,
root overlap and retained policy publication were reused; neither the public
protocol nor persisted representation gains fields. Reports:
`.fitness/portable-drives-ram-managed-local.json` and
`.fitness/portable-drives-ram-managed-branch.json`.

| Local architecture dimension | Before | After |
| --- | --- | --- |
| Average / maximum CCN | 3.49 / 274 | 3.49 / 274 |
| Duplication | 6.97% | 6.97% |
| Clone groups | 4,300 | 4,300 |
| Dependency cycles | 144 | 144 |

The original-baseline result remains REGRESSION: average CCN 3.44→3.49,
maximum 274 unchanged, duplication 7.30%→6.97%, eleven net clone groups and
cycles 143→144. This is a prerequisite liveness/alias fix; RAM resume/export
directory preparation and helper recovery still require worker handoff, and
watch/configuration/mount-control preparation, the wider lifetime matrix and
final workspace/architecture gates remain open. Items 1.5 and 1.6 stay Partial.

## Retained folder-enrollment preparation checkpoint

Folder enrollment now carries a fresh partition pin from read-only scheduling
into the existing four reconciliation workers. Scheduling also plans the local
destination region without creating it. Accepted preparation reserves both
regions and the drive until actual worker exit; preparing and copying share the
four-slot total. The worker selects the guarded source subtree, checks unchanged
destination scheduling evidence, creates/pins the local directory through the
existing core mechanism, validates managed storage and other enrollment targets,
and captures its local storage identity. The shared Root subtree helper reuses
the existing no-crossing directory walk and retains read-only requirements.

The coordinator publishes the pair only after the completed preparation still
matches managed storage, enrolled target addresses/identities, the observation
epoch and current mount signature. Changed inputs, cancellation or publication
failure cannot start the initial union. Created empty folders are retained when
later validation fails. The write worker repeats other-target overlap checks
until the first successful union, including after a scheduling deferral. Missing
enrolled targets now retain lexical boundaries through the existing managed-root
overlap mechanism. Publication and baseline mechanisms otherwise remain the same.
Preparation polling removes only delivered jobs, keeping other workers' region
reservations visible during admission.

The final-inventory replacement control
`/tmp/portable-drive-enrollment-final-root-before.log` exposed a further gap:
the write worker refused a replaced local directory, but the pair had already
been registered. Shared plan root/managed-storage validation now runs after the
last inventory call before returning preparation, and again before payload
work. The new control verifies the failure leaves no pair, preserves replacement
bytes and installs no source payload. This shared validation also removes the
duplicated write-path checks without changing their behavior.

The negative control `/tmp/portable-drive-enrollment-source-before.log` records
one coordinator inventory call during enrollment. The new zero-call expectation
fails before this change. Controlled cases cover success, cancellation/shutdown,
changed configuration/destination/drive, read-only refusal, forced registry-write
failure, overlapping concurrent enrollment and missing previous targets. While
the intended enrollment worker is held, configuration completes and another
pair resumes and syncs. The fixture pauses the earlier pair before arming its
one-shot gate so an unrelated automatic refresh cannot consume the gate.

Two earlier fixtures waited for the removed owner inventory call or treated a
warmed scope worker as unseen; their obsolete barriers timed out in the initial
backend runs. Queued-Save cancellation assertions now run with all four real
scope probes held: cancellation creates no directory and releases no probe
owner. Warmed workers are tracked independently for each armed phase. The real
FUSE scope fixture now includes enrollment alongside catalog and RAM requests;
the reconciliation fixture exercises both an enrolled-source stall and a new
enrollment destination stall after read-only scheduling has completed.

The first combined guest run failed at another removed enrollment-owner barrier
inside the real-overflow fixture. That failure left the guest's inotify limit
changed and contaminated later cases. The replacement runs a real owned daemon
subprocess with unchanged native drive facts and a one-hour fallback, waits for
initial union, then observes SIGSTOP before producing more than 64 alternating
file events. No daemon thread can drain that queue. SIGCONT allows the original
union/conflict/default-restoration checks; a rule publication observes unchanged
inventory before the later nested edit without running an explicit sync or
refreshing its watches. The child is bounded and killed/reaped on parent failure;
sysctl guards restore original limits. Its internal ignored probe is selected
only by the parent; grouped event-loss runs use `--skip inotify_owner_probe`.
RAM helper suspension and abrupt-daemon fixtures share the existing suspension
and child-cleanup behavior through test_child rather than duplicate it.

Current backend evidence: 237 passed, 96 explicit fixtures ignored in
`/tmp/portable-drive-enrollment-backend-final-roots.log`. Five further controlled
enrollment repetitions pass in `/tmp/portable-drive-enrollment-repeat-0.log`
through `-4.log`. Affected-package all-target Clippy, the runtime build and
changed-source rustfmt checks pass. Final guest evidence is
`/tmp/portable-drive-enrollment-vm-final-roots/report.json`: 99 main cases pass
across FAT32/exFAT/NTFS, with three further owned-daemon child probes (102 raw
passing test-result counts). It includes the final-root replacement case, both
FUSE preparation stalls, three source-scope stalls, RAM handoff/population,
native sync/backup/name handling and real event-loss recovery. The earlier
`/tmp/portable-drive-enrollment-vm-filesystems/report.json` remains a failed
fixture checkpoint; it is superseded rather than treated as passing evidence.
The host preparation/scope fixture logs also pass their selected cases.

Full dark UI acceptance passes in
`/tmp/portable-drive-enrollment-ui-dark/report.json` before the final root guard;
the final build's full light flow passes in
`/tmp/portable-drive-enrollment-ui-final-light/report.json`. Each exercises drive
menus, RAM writeback, UI union enrollment, lazy backups, subtree exclusions,
offline FUSE and preserved conflict review. Union/conflict screenshots were
inspected. The focused light storage/enrollment flow also passes in
`/tmp/portable-drive-enrollment-ui-light/report.json`.

Local architecture remains REGRESSION because the new preparation, publication
and negative-control cases add 490 NLOC, fourteen net functions and sixty total
CCN. `prepare_reconciliation` improves from CCN 23 to 10. `run_with_cancel` grows
19→20 for the initial-target overlap check; extracting validation for actual
reuse after inventory reduces the draft's CCN 23 to 20. New draft test clones
were removed by reusing Held's enrollment operation and dropping obsolete
first-observation gating. The five remaining modified-file clone reports are
all present in the baseline (four service tests and the root/copy fixture).
No new clone group or dependency cycle remains, and no public protocol or
dependency was added.

| Local architecture dimension | Before | After |
| --- | --- | --- |
| Average / maximum CCN | 3.49 / 274 | 3.49 / 274 |
| Duplication | 6.97% | 6.97% |
| Clone groups | 4,300 | 4,300 |
| Dependency cycles | 144 | 144 |

Local report: `.fitness/portable-drives-enrollment-final-roots-local.json`.
Original-baseline report:
`.fitness/portable-drives-enrollment-final-roots-branch.json` remains REGRESSION:
average CCN 3.44→3.49, maximum 274 unchanged, duplication 7.30%→6.97%, clone
groups 4,289→4,300 and cycles 143→144. This checkpoint narrows
1.5 and part of 1.6; the other unfinished rows, including watch registration,
RAM resume/export, configuration/mount controls, remain with the final
workspace/architecture gates.

## Retained RAM source handoff checkpoint

Copy to RAM now retains the source opened by read-only scheduling admission,
instead of reopening it and repeating inventory calls on the coordinator. Both
stable drive IDs and device selectors obtain fresh inventory on the existing
four scope workers. The resulting drive/root observation supplies the exact
scheduling claim checked by dispatch; deferral or a changed observation releases
the pin and retries ordinary admission. Accepted RAM preparation keeps that same
pin on the existing single preparation worker, with the established busy-drive
reservation, cancellation, shutdown and singleton ownership. No new pool or
source-opening mechanism was added.

Before eligibility, RAM availability, managed-root validation, the allocation
journal or source enumeration, the preparation worker rechecks fresh drive
identity and validates the retained root. Root validation follows inventory so
a directory replaced during a slow inventory call cannot pass the earlier
observation. The existing checks after sizing and allocation remain. RAM quota
configuration changes still queue while an accepted allocation owns its settings;
indexing-rule controls and independent pairs continue to run. Broader enrollment,
export/resume, configuration-directory, bulk/partition and mount-control
preparation remain tracked in 1.5.

The negative control `/tmp/portable-drive-ram-source-before.log` records two
coordinator inventory calls during one Copy to RAM request. Its new expectation
of zero calls fails before the change and passes afterward. Controlled held-worker
cases verify independent rule completion and sync, changed drive evidence,
replacement of the source directory during inventory, cancellation, retained
shutdown/singleton ownership, and absence of preflight rows or allocation journals
after refusal. The stopped-FUSE scheduling fixture now covers both catalog capture
and stable-ID Copy to RAM, with observed non-coordinator kernel waits, responsive
configuration, independent writeback and actual-exit shutdown ownership. Shared
one-shot named-worker gating reuses the existing bounded gate in RAM and sync
tests, eliminating the first draft's new test clone.

Validation: 233 backend tests pass (95 explicit fixtures ignored) in
`/tmp/portable-drive-ram-source-backend-final.log`; affected-package all-target
Clippy passes with warnings denied in `/tmp/portable-drive-ram-source-clippy.log`.
The owned native scope run passes all five selected cases, including both stopped
FUSE requests and simultaneous RAM-setup deliveries, in
`/tmp/portable-drive-ram-source-native-scopes.log`. The final guest run passes 75
cases across FAT32/exFAT/NTFS:
`/tmp/portable-drive-ram-source-vm-final/report.json`. It includes direct service
partition-to-RAM population, paused RAM-first edits flushed back to the selected
partition, cancellation/replacement controls, both initial-population recovery
cases, sync and event-loss recovery. The new service fixture reuses TestDrives
rather than duplicating startup. Private light UI acceptance passes RAM setup,
removal, offline export and reconnect, with the reconnected status screenshot
inspected: `/tmp/portable-drive-ram-source-ui-light/report.json`. The runtime
build and changed-source rustfmt checks pass.

The local architecture comparison still reports REGRESSION because total CCN
increased by 11 across the new cases and source handoff. It introduces no new
reported hotspot, clone group or dependency cycle. `start_ram_setup` falls from
CCN 17 to 11 by removing coordinator inventory/root work and its duplicated
admission checks; actual source opening and validation reuse the existing scope
and RAM preparation workers. All four reported service-test clone fragments
were already present in `pre-ram-source-preparation`. The shared named gate and
existing TestDrives fixture remove the two draft test clones without altering
the assertions. Added code is 211 NLOC and four net functions, primarily test
coverage; no new public API or dependency is introduced.

| Local architecture dimension | Before | After |
| --- | --- | --- |
| Average / maximum CCN | 3.49 / 274 | 3.49 / 274 |
| Duplication | 6.98% | 6.97% |
| Clone groups | 4,300 | 4,300 |
| Dependency cycles | 144 | 144 |

Reports: `.fitness/portable-drives-ram-source-local.json` and
`.fitness/portable-drives-ram-source-branch.json`. Against the original baseline,
the unresolved branch result remains REGRESSION: average CCN 3.44→3.49, maximum
274 unchanged, duplication 7.30%→6.97%, eleven net additional clone groups and
cycles 143→144. This checkpoint narrows item 1.5; it does not close the other
audit rows or the final architecture gate.

## Managed catalog/image boundary checkpoint

The previous status-page lookup could miss a mounted legacy catalog or image
header after 1,000 newer history records. Negative controls in
`/tmp/portable-drive-managed-view-before.log` reproduce both the missing legacy
boundary and acceptance of its managed source. The replacement lookup combines
mounted/uncertain catalog and backup headers with all current registrations in a
single transaction. A partial SQLite index avoids decoding or walking ordinary
unmounted history for this lookup; 128-key pages, bounded identifiers, 64 KiB
headers and lossless path decoding are shared with the existing Store mechanisms.
The query-plan test proves index selection after migrating legacy state, and
checks updates, rollback, malformed candidates, deletion and oversized keys.
Header/runtime image-path disagreement retains both deny boundaries. Missing,
oversized, malformed, relative, parent-traversing and NUL-containing registered
paths fail closed. These metadata boundaries never authorize provider reads or
unmounting.

Sync source/target preparation, backup destinations, RAM setup and RAM export
reuse the full view boundary lookup. The RAM setup worker and export worker
repeat the checks before payload enumeration. An export whose already-pinned
destination becomes a managed view is refused before scanning or writing; the
existing cancellation/completed-file and retry/metadata checks remain intact.
Catalog header and registration retirement now share a transaction after both
normal unmount and detection of an external unmount. A forced update failure
proves rollback retains both records, followed by a successful retry.

Policy metadata-capture failures immediately publish a root exclusion through
the retained publication worker. A known metadata error never reaches its
filesystem observer. Rule completion reports the pause; repair and an explicit
retry restore the verified policy while preserving prior offline observations.
Catalog FUSE source labels in the same bounded namespace snapshot provide
additional deny-only boundaries for bind aliases, including after origin loss.
Those literal exclusions bypass FUSE getattr/root probes. The first draft's
17 backend failures exposed disconnected catalog servers already present in the
host namespace; no unknown mount was removed. Avoiding those probes allows
unrelated policy publication to proceed without making the mount label content
authority. Orphan RAM/image aliases and the wider managed-root/lifetime matrix
remain unfinished; plan item 1.6 stays Partial.

Current backend evidence: 231 passed, 94 explicit fixtures ignored in
`/tmp/portable-drive-managed-view-backend-final.log`; affected-package all-target
Clippy passes with warnings denied in
`/tmp/portable-drive-managed-view-clippy.log`. The native filesystem run passes
117 cases across FAT32/exFAT/NTFS in
`/tmp/portable-drive-managed-view-vm-filesystems/report.json`, including real
catalog bind-alias protection after original-mount removal, catalog worker/limit
and publication lifecycle checks, sync and event-loss recovery. The final-build
image/lifecycle run passes another 36 cases, including the managed-view and
atomic-retirement checks on all three formats, real UDisks image mounts, managed
image-root refusal, provider revision checks and live maintenance recovery:
`/tmp/portable-drive-managed-view-vm-lifecycle/report.json`. Private light UI
acceptance passes RAM setup, pending changes, removal, offline export and
reconnect in `/tmp/portable-drive-managed-view-ui-light/report.json`; offline and
reconnected status screenshots were inspected. Changed Rust sources pass
rustfmt checks and the runtime build succeeds. This checkpoint does not close
the remaining audit rows or the branch-wide architecture gate.

Architecture comparison against `pre-managed-view-admission` still reports
REGRESSION. The added admission validation and atomic retirement each add one
fallible step to existing routines: `start_ram_setup` 16→17, export `copy` 16→17,
and `unmount` 17→18. Consolidating retirement into one transaction removed the
first draft's duplicated cleanup and redundant observed-mount branch (20→18).
The lookup reuses Store paging/bounded decoding, existing pinned-root overlap
checks and the existing retained policy publisher; it introduces no dependency
or payload-access mechanism. The five reported modified-file clone fragments
were individually checked against the saved baseline: all were already present
in `store::list`/`scan::page` or existing service tests. Total clones/cycles do not
grow in this checkpoint. The new paths/tests account for 577 NLOC, 21 functions
and 69 total CCN; the three necessary error checks remain disclosed rather than
being hidden behind metric-only wrappers.

| Local architecture dimension | Before | After |
| --- | --- | --- |
| Average / maximum CCN | 3.49 / 274 | 3.49 / 274 |
| Duplication | 6.98% | 6.98% |
| Clone groups | 4,300 | 4,300 |
| Dependency cycles | 144 | 144 |

Reports: `.fitness/portable-drives-managed-view-local.json` and
`.fitness/portable-drives-managed-view-branch.json`. Against the original
`pre-portable-drive-services` baseline, the unresolved result remains REGRESSION:
average CCN 3.44→3.49, maximum 274 unchanged, duplication 7.30%→6.98%, clone groups
4,289→4,300 and cycles 143→144. These broader remaining regressions and the
unfinished audit rows still require work.

## Retained reconciliation-root preparation checkpoint

Enrolled sync and RAM-writeback root preparation now runs on the existing four
reconciliation workers, with preparation and copying sharing a four-slot total.
The coordinator captures only registry/settings data and retained cache references.
The worker applies the existing media identity/read-only, RAM ownership,
ordinary-directory, local identity, managed-storage and cached-root checks; fresh
inventory and an unchanged mount signature bind its result. Before dispatch the
coordinator compares the exact enrollment/RAM owner and managed-path snapshot,
checks the observation epoch and current watch coverage, and repeats scheduling
admission. Stale inputs requeue without copying. The write worker checks the
captured managed boundaries again before enumeration or installation, covering a
path replaced after preparation. Existing common-baseline and copy mechanisms are
unchanged.

Preparing jobs reserve their drive and retain their intent, pins and worker slot
through actual exit. Pause cancels automatic preparation; shutdown cancels/drains
both preparation and writes before releasing the singleton. Accepted operations
in scheduling admission or the deferred queue take priority over automatic
preparation. The first draft's two catalog/image timeout failures exposed an
automatic-preparation starvation loop; checking that priority before preparation
and after its return removes it. Paused automatic intents are dropped before
performing filesystem work. Pending scheduling/preparation phases use a 20 ms
coordinator receive wait instead of adding a 200 ms idle wait per phase; idle and
long-running-copy behavior retain the prior wait. The initial native lifecycle
run's NTFS retirement check caught this added latency: its real cleanup had
already finished before independent controls completed
(`/tmp/portable-drive-sync-prepare-vm-lifecycle/report.json`). The unchanged live
retirement test passes on the host after the shorter admission wait
(`/tmp/portable-drive-sync-prepare-prune-responsive.log`). Its workload and
live-cleanup/cancellation assertions were not weakened.

Commit `d0a7143b` extends the existing executor with optional role names and makes
retained drive state comparable without changing serialization. Existing unnamed
executors keep their behavior. Storage pools use role names, and controlled
inventory fixtures now distinguish status/policy/scheduling/preparation work.
The overflow fixture deliberately holds an explicit enrollment stage on a
separate ordinary source, so the coordinator really stops draining inotify;
background sync inventory is no longer mistaken for a coordinator barrier. The
watcher-unavailable query assertion counts the moved sync-worker queries too.

The backend passes **224 tests**, with 93 opt-in tests ignored
(`/tmp/portable-drive-sync-prepare-backend-responsive.log`). New ordinary cases prove
independent sync/configuration during held preparation, retained cancellation and
singleton ownership through shutdown/restart, rejection of a plan after managed
settings change, and recovery after restoring those settings. A direct write-worker
test replaces an admitted backup path with a source alias and proves refusal
before inventory rows or destination files are created. The focused suite passes
all three selected cases, including a real stopped owned FUSE source with kernel
wait-channel evidence for the named sync worker
(`/tmp/portable-drive-sync-prepare-focused-responsive.log`). Four real FUSE-stall
scenarios, all three scheduling-admission cases and retained RAM preflight also
pass (`/tmp/portable-drive-sync-prepare-stalls.log`,
`/tmp/portable-drive-sync-prepare-admission.log`,
`/tmp/portable-drive-sync-prepare-ram-preflight.log`). Core passes **467 tests**
and **15 doctests**; its generated executor API documentation was checked
(`/tmp/portable-drive-sync-prepare-core.log`,
`/tmp/portable-drive-sync-prepare-core-docs.log`,
`/tmp/portable-drive-sync-prepare-rustdoc.log`). Four-package all-target Clippy
passes with warnings denied and the current runtime build succeeds
(`/tmp/portable-drive-sync-prepare-clippy-responsive.log`,
`/tmp/portable-drive-sync-prepare-runtime-responsive.log`).

The final native rerun passes **135 cases** across FAT32, exFAT and default NTFS3:
45 filesystem/sync cases, six live event-loss/watcher recovery cases, 57
catalog/limit/lifecycle/policy cases and 27 partition/backup/provider lifecycle
cases. Reports are
`/tmp/portable-drive-sync-prepare-responsive-vm-sync/report.json`,
`/tmp/portable-drive-sync-prepare-responsive-vm-event-loss/report.json`,
`/tmp/portable-drive-sync-prepare-responsive-vm-catalogs/report.json` and
`/tmp/portable-drive-sync-prepare-responsive-vm-lifecycle/report.json`.
The last report includes the unchanged live-retirement test that failed before
the shorter admission wait. These runs use LTS 6.18 and leave the separate
CachyOS 7.3.4 exFAT durability issue open. Final light-theme RAM acceptance passes
online writeback, removal, retained pending changes, offline export and reconnect
(`/tmp/portable-drive-sync-prepare-ui-responsive-light/report.json`); offline and
reconnected status screenshots were inspected. Formatting and diff checks pass.

Compared with `pre-sync-root-preparation`, clone groups remain 4,300, cycles 144,
average CCN 3.49 and maximum 274; duplication decreases from 6.99% to 6.98%.
The checkpoint adds 644 NLOC, 30 functions and 80 total CCN, including regression
fixtures. Shared fixture/shutdown invariants and simpler task retirement remove
the draft's eight extra clone groups. The old storage-validation hotspot is
resolved. No new hotspot or cycle remains; `run_with_cancel` increases from CCN
17 to 19 for the necessary write-time managed-path check. The comparison still
reports REGRESSION for aggregate complexity and unchanged service fragments/cycle
member ordering; the four flagged service fragments were verified present in
the starting revision. The original whole-branch gate remains open.

This advances 1.5 without closing it. Pair enrollment, RAM resumption/export
preparation, bulk/partition preparation, review/configuration-directory validation,
watch registration, mount controls and startup/recovery retain synchronous stages.
The managed-storage snapshot preserves existing catalog-header coverage; full
registered image/catalog-view admission beyond that bounded historical-header
lookup remains with 1.6. No responsiveness or managed-root requirement is marked
complete from these scoped checks.

## Retained scheduling-admission checkpoint

Queue scheduling no longer opens removable source roots or plans RAM export
destinations on the coordinator. Four retained `BackgroundJobs`/`Task` slots own
these read-only probes and device-node selector resolution. Registry-only drive
lookup stays local. Probes neither create output directories nor authorize a
write: dispatch repeats current busy/overlap checks, and the existing fresh
execution preparation remains mandatory. Each result binds an unchanged mount
inventory; a changed owner observation epoch or mount table before dispatch
causes fresh admission. In-probe mount changes fail without starting a mutation.
Unavailable source hints retain the prior behavior of leaving definitive
validation to execution preparation.

Cancellation retains accepted IDs and occupied probe slots through actual task
exit. Shutdown cancels and drains probes before storage cleanup, retaining the
singleton. Probe saturation does not block local configuration or an already
enrolled independent pair. RAM setup is checked both before probing and when a
result returns, so a concurrent result cannot replace the single preparation
owner; the early check also avoids repeating probes while that slot is busy.
The existing reconciliation preparation/queue step now lives with its module,
and routing uses one request match rather than repeated branch predicates.

The ordinary suite passes **221 tests**, with 92 opt-in cases ignored
(`/tmp/portable-drive-scope-backend-verified.log`). All three new admission
scenarios pass (`/tmp/portable-drive-scope-all-v3.log`). Four deliberately held
device-inventory probes leave a setting change and another pair responsive;
cancelling all four does not admit a fifth request until one actually exits.
The missing-session export then fails without creating its parent. The remaining
three probes retain singleton ownership through shutdown and persist all four
cancelled results without a catalog. A real stopped owned FUSE source supplies
kernel wait-channel evidence for the scheduling worker while settings and another
pair continue, then verifies cancellation/shutdown and unchanged source bytes.
Two simultaneous Copy-to-RAM probes cannot replace the first setup owner. This
case cancels before image allocation, leaves no setup journal/session, and passes
three additional repetitions
(`/tmp/portable-drive-scope-ram-serialization-repeat.log`). Four-package all-target
Clippy passes with warnings denied and the runtime build succeeds
(`/tmp/portable-drive-scope-clippy-verified.log`,
`/tmp/portable-drive-scope-runtime-verified.log`).

The native catalog matrix passes **57 cases**, 19 each on FAT32, exFAT and default
NTFS3 (`/tmp/portable-drive-scope-vm-catalogs/report.json`). It exercises live
worker-limit changes, independent/overlapping captures, queued backup exclusivity,
concurrent shutdown, explicit catalog limits, all ten capture lifecycle cases and
three bind/case/unmount policy cases. Private dark-theme RAM acceptance passes
online writeback, removal, retained pending changes, offline export and reconnect
(`/tmp/portable-drive-scope-ui-ram-dark/report.json`); pending and reconnected
status screenshots were inspected. These checks use the current runtime build.
The separate partition/backup/provider lifecycle run passes **27 cases**, nine
per format (`/tmp/portable-drive-scope-vm-lifecycle/report.json`), including
retirement/recovery responsiveness, revision-checked providers, nested catalog
refusal, external-write preflush and authorized image backup/busy refusal.
Both native runs use LTS 6.18 and leave the separate CachyOS 7.3.4 exFAT durability
issue open. Targeted formatting and diff checks pass.

Against `pre-drive-scope-admission`, clone groups remain 4,300, cycles 144,
duplication 6.99%, average CCN 3.49 and maximum 274. The implementation and
regression fixtures add 459 NLOC, 15 functions and 61 total CCN. The initially
measured probe hotspot and duplicated timing assertion were removed; no new
hotspot, clone or cycle remains. The existing operation hotspot drops from CCN 28
to 6, while the coordinator run loop stays at 26. The tool still reports
REGRESSION for total CCN growth and unchanged service-test fragments/member-order
changes; each flagged service clone was verified present in HEAD. The original
whole-branch architecture review remains open rather than being reset.

This advances 1.5 without closing it. Fresh reconciliation/RAM/bulk/partition
preparation, review validation, configuration-directory validation, mount controls
and startup/recovery still have synchronous stages needing independent work and
responsiveness evidence. This checkpoint does not claim that every operation's
admission is now asynchronous.

## Retained status-refresh checkpoint

Periodic discovery and operation-completion refreshes now use one retained
`BackgroundJobs`/`Task` worker. Same-input refreshes coalesce; changed inputs retain
one latest desired configuration. The owner keeps servicing controls and other
pairs while inventory or RAM-availability metadata is blocked. A changed RAM
directory/minimum shows a pending observation, and an older configuration's
availability result cannot replace it. Status is observational: fresh operation
admission remains independent, and a completed mount/unmount may precede its
status refresh.

Policy observation independently obtains fresh drive facts, bound to an unchanged
mount-inventory signature. It cannot reuse a stale status source to reopen
indexing. Failed inventory or a changed mount context keeps the root exclusion
and previous offline observations; an explicit retry establishes fresh evidence.
Shutdown retains active probes through actual exit and obtains a final
post-cleanup observation. Finishing a held rule during the later publication
drain cannot start an unowned late status probe. Singleton ownership and the
accepted rule's durable completion remain intact through that sequence.

Save-RAM queue scheduling now plans a missing destination's region from its
existing ordinary ancestor without creating directories. This scheduling scope
does not invent destination object identity or authorize writes. Execution keeps
the existing guarded creation and enrollment checks. Missing siblings retain
conservative name-equivalence overlap; existing aliases and file ancestors are
tested. An unknown RAM session cannot create its requested output parent.

The final ordinary backend suite passes **220 tests**, with 90 opt-in cases
ignored (`/tmp/portable-drive-refresh-backend-shutdown-final.log`). Core passes
**467 tests**, with four ignored
(`/tmp/portable-drive-refresh-ordinary-final.log`). Two real stopped-FUSE tests
pass (`/tmp/portable-drive-refresh-stalls-final.log`): kernel wait-channel evidence
proves the policy and RAM-availability workers actually block while configuration
and another pair remain responsive. The RAM test also holds the next observation
to prove the stale failure was rejected before a good result can hide it. Twelve
held-refresh configuration changes coalesce into two observations; a moved source
still receives its fresh no-index policy while displayed inventory remains old.
Four-package all-target Clippy passes with warnings denied
(`/tmp/portable-drive-refresh-clippy-shutdown-final.log`), and the runtime build
passes (`/tmp/portable-drive-refresh-runtime-final.log`).

Nine native bind/case/unmount policy cases pass across FAT32, exFAT and default
NTFS3 (`/tmp/portable-drive-refresh-vm-aliases/report.json`). The first combined
invocation incorrectly appended them after image restoration had moved the
source mount; its alias failures are retained at
`/tmp/portable-drive-refresh-vm-final/report.json` and do not count as native
coverage. The separate alias run uses its original verified mount fixture.
The clean standalone lifecycle run passes all **27 cases**, nine per format,
covering retirement/recovery responsiveness, original/backup provider validation,
nested catalog refusal, external-write preflush and authorized partition-image
backup (`/tmp/portable-drive-refresh-vm-lifecycle/report.json`). Both successful
guest runs use LTS 6.18; they do not close the separately tracked CachyOS 7.3.4
exFAT durability issue.
Private UI acceptance passes explicit metadata measurement in dark appearance
(`/tmp/portable-drive-refresh-ui-storage-dark/report.json`) and RAM pending
changes, removal, offline export and reconnection in light appearance
(`/tmp/portable-drive-refresh-ui-ram-light/report.json`). The rendered report,
offline pending status and reconnected confirmation were inspected.

Compared with `pre-drive-admission-responsiveness`, clone groups remain 4,300,
cycles 144, duplication 6.99%, average CCN 3.49 and maximum 274. The implementation
and regression fixtures add 660 NLOC and 28 functions. No refresh or modified
policy-test function exceeds CCN 15. The comparison still reports REGRESSION:
total CCN grows by 91, the flagged root/service clone fragments already exist in
HEAD, and the reported Python cycles only change member ordering. The original
`pre-portable-drive-services` comparison also remains REGRESSION (11 additional
clone groups and one cycle across the whole branch); it is not reset or waived.

This advances 1.5 without completing it. Initial discovery/recovery, fresh
source/destination admission, configuration backup-directory validation,
image/catalog controls and broader cleanup scheduling remain synchronous stages
requiring their own responsiveness evidence. Physical-device interruption,
provider lifetime and remaining no-index consumer matrices remain open.

## Historical catalog name-resolution checkpoint

The native control reproduced silent successful cleanup with retained descendants
for `Future/future` on FAT32 and exFAT. Default NTFS3 correctly kept those names
distinct. Evidence: `/tmp/portable-drive-redaction-alias-before/report.json` and
`guest.log`. A separate pinned-descriptor probe confirmed the native filesystem
returns its canonical directory spelling without enumerating children
(`/tmp/portable-drive-redaction-fd-name-probe/report.json`).

New captures optionally record the canonical source path and core's existing
`MountLifetime` in their local SQLite metadata. Exact saved paths and lexical
legacy normalization retain offline cleanup. A missing exact path instead needs
that observed original: the resolver forbids symlink traversal, validates boot,
namespace, object and non-reused mount identity, and checks the rule again before
committing. A canonical saved directory takes the indexed direct path. Otherwise
the existing bounded scan cursor checks recorded paths at the rule's depth
against current metadata, without reading file payloads or guessing collation.
The same SQLite transaction covers every resolved boundary and its durable
marker, so uncertainty rolls back earlier edits. Reader leases and compaction
remain with the existing redaction owner. No source-side metadata is introduced.

Unavailable/stale originals, unresolved saved names and older catalogs without
this context retain their cleanup intent and report an error instead of silently
completing. A newer mount cannot borrow an earlier lifetime. An explicitly chosen
whole-drive catalog exclusion still works offline and lets older uncertain
intents finish once no entries remain. This is a recovery option, not an automatic
widening of a folder exclusion. Arbitrary offline alias resolution remains open;
this checkpoint does not close 6.2–6.4 or legacy/remount/relocation coverage.

The ordinary backend suite passes **214 tests**, with 89 opt-in tests ignored
(`/tmp/portable-drive-redaction-alias-backend-final.log`). Seven new ordinary
tests cover a raw-name file with unreadable payload permissions, late aliases,
unavailable-source recovery, explicit offline root cleanup, rollback after an
unresolvable sibling, missing/forged mount context, symlink replacement, rule
retargeting and cancellation. Replay checks compare the entire retained catalog
header, including its original date and metadata generation. Backend all-target
Clippy passes with warnings denied
(`/tmp/portable-drive-redaction-alias-clippy-final.log`).

The final native report passes **six selected tests**, two each on FAT32, exFAT
and default NTFS3 (`/tmp/portable-drive-redaction-alias-final/report.json`). Four
ASCII/Unicode/nested name pairs follow the kernel's actual equivalence in each
format. Every pair refuses an intervening source bind replacement. Equivalent
directory cases remove an unrelated source sibling after capture, proving the
canonical path does not need to probe it. The case-only rename path runs on
FAT32/exFAT; default NTFS3 explicitly reports that those names are distinct.
A separate NTFS3 `nocase` run passes both tests, all four equivalent pairs and
the rename path (`/tmp/portable-drive-redaction-alias-ntfs-nocase/report.json`).
FAT32/exFAT in that separate run are setup-only, not extra test cases. The real
FUSE legacy-reader retention test also passes
(`/tmp/portable-drive-redaction-alias-fuse.log`). These guest runs use LTS 6.18
and do not resolve the separately tracked CachyOS 7.3.4 exFAT durability issue.

Architecture retains 4,300 clone groups and 144 cycles, average CCN 3.49, maximum
274 and 6.99% duplication. Splitting source admission, canonical-directory matching
and the bounded fallback removes the initially measured CCN 23 resolver hotspot.
The replay assertion reuses one full-header invariant. No new clone group, cycle
or redaction hotspot remains; whole-branch REGRESSION is still open.

The UI follow-up shows an unmount suggestion only while a catalog actually has
a mounted view. Unmounted cleanup failures retain their reason. The settings
completion notice now directs people to catalog status instead of implying
unmounting alone guarantees cleanup. Fifteen Files drive/policy presentation
tests pass (`/tmp/portable-drive-redaction-alias-files-tests.log`); the final
backend rerun passes the same 214 tests
(`/tmp/portable-drive-redaction-alias-backend-notice-final.log`). Four-package
all-target Clippy passes with warnings denied
(`/tmp/portable-drive-redaction-alias-clippy-notice-final.log`), and the final
runtime build succeeds (`/tmp/portable-drive-redaction-alias-runtime-notice-final.log`).

The extended private redaction scenario passes in dark and light appearances:
`/tmp/portable-drive-redaction-alias-ui-extended-dark/report.json` and
`/tmp/portable-drive-redaction-alias-ui-final-light/report.json`. It retains a real
FUSE reader through a refused unmount, reopens the redacted view, physically moves
the owned source offline, observes the unresolved alias and unchanged entry/date
counts, and shows its actual error without an unmount instruction. An explicit
whole-drive exclusion then clears all pending intents and displays zero entries;
the source's original bytes survive restoration. The light run includes the final
completion-notice wording; the dark run precedes that wording-only adjustment.
Pending and cleared screenshots were inspected in both appearances, with the
cleared light view explicitly checked for the absence of the pending message.
Targeted Rust formatting, Python syntax and diff checks pass. No new clone group,
cycle or UI-harness hotspot is introduced by this follow-up.

## Retained policy-observation checkpoint

Policy publication no longer resolves removable-media metadata on the coordinator.
One retained `BackgroundJobs`/`Task` observer owns mount routing and temporary
source pins; only the coordinator installs its current result through the existing
guarded atomic policy writer. A changed input first publishes a root exclusion,
retaining the last descriptor-free offline-owner observations. One latest desired
input replaces intermediate requests. Stale results cannot reopen indexing or
complete a newer rule, and an explicit rule retry requires a fresh observation
even if its saved flags are unchanged. Observation failure keeps indexing paused.

Rule edits operate on local state without generic source pin admission. At most
16 applied rules await policy publication; further operations use the existing
bounded deferred queue. Cancellation before application prevents the edit, while
cancellation after application retains its actual result. A successful rule is
not reported complete until its policy has been installed. Shutdown retains the
observer, accepted rule records and singleton until the task actually exits.

The ordinary backend suite passes **207 tests**, with 87 opt-in cases ignored
(`/tmp/portable-drive-policy-worker-backend-final.log`). Core passes **467 tests**,
with four opt-in cases ignored
(`/tmp/portable-drive-policy-worker-ordinary-final.log`). Controlled end-to-end
tests hold the observer while configuration and independent sync complete, apply
16 rules while skipping intermediate observation backlogs, cancel a queued rule
without saving it, retain an applied rule after cancellation, and verify shutdown,
singleton exclusion, restart and replay. A failed same-input retry control caught
sharing of an older observation; the fresh-observation requirement fixes it.

The opt-in native test suspends only its owned, unreaped FUSE helper and confirms
the policy worker's actual kernel wait (`wchan=fuse_chan_send`). While the helper
is stopped, configuration completes within two seconds and a separate pair
copies an edit. The rule remains unfinished behind the root exclusion; resuming
the child completes publication and the rule. A scoped guard resumes the child
on unwinding. Evidence: `/tmp/portable-drive-policy-worker-native-final.log`.

Four-package all-target Clippy passes with warnings denied
(`/tmp/portable-drive-policy-worker-clippy-final.log`), and the file-manager,
CLI, catalog/mount helpers and fixture build successfully
(`/tmp/portable-drive-policy-worker-runtime-build.log`). The rebuilt application
passes private light policy-editor acceptance, including unchanged saves beyond
status pagination, and dark Jobs/storage acceptance with sync bookkeeping
separate from optional catalogs. Reports and inspected screenshots are in
`/tmp/portable-drive-policy-worker-policy-light/` and
`/tmp/portable-drive-policy-worker-storage-dark/`. The LTS guest also passes all
nine native bind/future-case/origin-removal policy cases across FAT32, exFAT and
NTFS3 (`/tmp/portable-drive-policy-worker-vm-aliases/report.json`).
The retained-maintenance/catalog-provider/authorized-image bundle also passes
**27 cases**, nine per FAT32, exFAT and NTFS3 GPT partition, including live
independent sync during retirement, startup recovery, FUSE revision checks,
flush/remount durability and UDisks image publication/restoration. Evidence:
`/tmp/portable-drive-policy-worker-vm-lifecycle/report.json` and `guest.log`.
This LTS acceptance does not resolve the separate CachyOS 7.3.4 exFAT issue or
the cross-driver precision limitation. Targeted formatting and diff checks pass.

Architecture remains at 4,300 clone groups, 144 cycles, average CCN 3.49, maximum
274 and 7.00% duplication. The policy lifecycle has its own module and reuses the
existing task, guarded writer and operation executor. The generic operation
dispatcher decreases from CCN 29 to 28. The run loop increases from 25 to 26 for
the required retained shutdown drain. No new clone group, cycle or policy-module
hotspot is introduced; the whole-branch REGRESSION remains open.

This closes the demonstrated policy-observation stall, not the full coordinator
responsiveness requirement. Discovery, startup recovery, source/destination
admission, image/catalog mount controls and remaining local-state work still
need their separate matrices. Historical offline case aliases and broader
consumer/relocation coverage remain open. Items 1.5, 1.6 and 6.2–6.4 remain Partial.

## Legacy offline redaction spelling checkpoint

A control reproduced silent cleanup failure for an older accepted intent using
`private//./nested/`: the operation completed but left the canonical saved
descendant row. Evidence: `/tmp/portable-drive-legacy-redaction-before.log`.
The attempted `.` control failed input validation during setup; it is not
evidence of silent root-redaction failure and does not change root-address rules.

Redaction now derives SQL boundaries and ancestor updates through the same
`Path::components` lexical normalization used for current rule addresses. The
accepted intent ID and durable replay marker remain unchanged. The regression
test uses a raw non-UTF-8 name, makes the original source path unavailable before
cleanup, verifies the excluded boundary and removed descendants, checks preserved
source bytes, and replays the original intent without a second metadata revision.
No source lookup, collation guess or new normalization helper is introduced.

The full ordinary backend suite passes **205 tests** with 86 opt-in tests ignored
(`/tmp/portable-drive-legacy-redaction-verified.log`); backend all-target Clippy
passes with warnings denied (`/tmp/portable-drive-legacy-redaction-clippy.log`).
Architecture retains 4,300 clone groups and 144 cycles, average CCN 3.49, maximum
274 and 7.00% duplication, with no new hotspot from this fix. Whole-branch
REGRESSION remains open. Targeted formatting and `git diff --check` pass.

Filesystem case aliases in historical offline snapshots are still a separate
gap: lexical normalization does not supply an old volume's name-equivalence
table. Items 6.2–6.4 remain Partial.

## Catalog capture name-equivalence checkpoint

A separate native control reproduced the same lexical bypass in catalog metadata
capture (`/tmp/portable-drive-vm-catalog-case-before/report.json`): FAT32/exFAT
descended into a future folder excluded using another spelling. Default NTFS3
correctly treated the tested spellings as distinct. This test uses `catalog=false`
with `index=true`; it does not depend on search-index exclusion.

The streaming scanner now compares the current rule object's device/inode with
the observed boundary entry through `Root::metadata`, at the rule's component
depth and before hashing or directory descent. Existing literal prefix matching
remains first. The existing NO_XDEV/no-symlink source-root checks still govern
every rule lookup, and cancellation is checked between lookups. No case-fold
table, directory-wide list, new content cache or persistent inode is introduced.
These lookups run only for configured catalog exclusions; sync and full backup
inventories retain empty exclusion lists. This matcher is separate from the
shared background-index policy because the two flags are independent and the
scanner retains a guarded source root with stricter no-symlink traversal.

The reviewed disposable run passes **seven native tests**: two each on FAT32,
exFAT and default NTFS3, plus the new four-pair capture test on NTFS3 `nocase`.
Each equivalent folder contains 32 hidden files with a 16-entry capture cap.
Equivalent boundaries are marked excluded, no descendants or hashes are saved,
ordinary metadata is present and at most three entries are consumed. Distinct
names remain eligible and reach the explicit cap. The companion existing test
also verifies full backup and enrolled sync inclusion on each source format.
Report: `/tmp/portable-drive-vm-catalog-case-reviewed/report.json`; the earlier
green six-test run before fixture consolidation is
`/tmp/portable-drive-vm-catalog-case-final/report.json`.

The ordinary backend suite passes **204 tests**, with 86 opt-in tests ignored
(`/tmp/portable-drive-catalog-case-backend-final.log`). Architecture review found
a duplicated native capture setup; the native fixture now owns its shared
guarded-source/state/capture sequence. The final comparison returns to 4,300
clone groups and 144 cycles, average CCN 3.49, maximum 274 and 7.00% duplication.
No new hotspot is reported here; the whole-branch regression remains open.

Capture-side matching does not prove historical offline redaction matching.
That matching is still lexical, and wider replacement races, alias/driver
matrices and policy-observation responsiveness remain open. Items 6.2–6.4 remain
Partial. Backend all-target Clippy passes with warnings denied
(`/tmp/portable-drive-catalog-case-clippy.log`); targeted Rust formatting and
`git diff --check` pass.

## Future-name equivalence checkpoint

After `ee3f397e`, a new disposable-guest control reproduced a search-index policy
bypass: save an exclusion for nonexistent `private/Future`, create
`private/future/secret`, and inspect admission without another publication.
FAT32 and exFAT resolved the two spellings to the same directory but allowed the
file; default NTFS3 treated them as distinct and correctly allowed it. Evidence:
`/tmp/portable-drive-vm-future-case-before/report.json` and `guest.log`.

The shared matcher now resolves the current rule through its known origin view,
validates that mount lifetime, and compares it with the candidate's corresponding
ancestor using pinned kernel identities. It persists no extra inode and reads no
payload or directory listing. Delete/recreation follows the current path, rather
than retaining an obsolete inode. A host test proves same-level hard-link
equivalence, release after deletion and matching the replacement object. Existing
literal prefixes remain the inexpensive first check. Metadata uncertainty and
ancestry outside the accessible candidate view continue to fail closed.

First-time rules with only partial views cannot prove disjointness from lexical
case alone. Those uncertain views are now excluded in full; routing retains the
precise projection when an original whole-volume view is available. The native
origin-removal test covers exact and alternate rule spellings without an older
policy. This conservative outcome is exposed in the user documentation.

Final native acceptance passes **nine tests**, three each on FAT32, exFAT and
default NTFS3 (`/tmp/portable-drive-vm-future-case-final/report.json`). The new
test contains four name pairs: `Future/future`, `Résumé/résumé`,
`Ångström/ångström` and `Future/Inner` versus `future/inner`. Each checks both
kernel-reported equivalence and ordinary-file allowance. Under these actual
mount settings FAT32 does not fold the Å pair, whereas exFAT does; default NTFS3
folds none. A separate NTFS3 `nocase` run passes all four equivalences
(`/tmp/portable-drive-vm-future-case-ntfs-nocase/report.json`). The other two
filesystems are setup-only in that separate run, not additional test cases.

Ordinary suites pass **204 backend tests** (85 opt-in ignored) and **467 core
tests** (four opt-in ignored), in
`/tmp/portable-drive-case-alias-ordinary-final.log`. Four-package all-target
Clippy passes with warnings denied (`/tmp/portable-drive-case-alias-clippy.log`).
Files policy consumers pass 19 tests
(`/tmp/portable-drive-case-alias-files-tests.log`); the owned Btrfs subvolume
fixture also passes the updated ordinary-file allowance checks
(`/tmp/portable-drive-case-alias-btrfs.log`). Targeted formatting and
`git diff --check` pass.
Architecture retains 4,300 clone groups, 144 cycles, average CCN 3.49 and maximum
274; duplication is 7.00%. Separating current-rule lookup/lifetime validation
from candidate-ancestor comparison removes the initially reported CCN 17 hotspot
without duplicating the kernel pin/identity mechanisms. No new hotspot is
reported for this checkpoint; whole-branch REGRESSION remains open.

This evidence does not close 1.6, 6.2 or 6.3. Saved catalog metadata matching and
redaction, wider relocation/consumer coverage, observation responsiveness and the
remaining driver/mount-option matrices still need their own evidence.

## Observed no-index alias checkpoint

Commit `c0ea5aad` moves the existing four-field mount lifetime into core, preserving
its serialized representation for image views and sync fingerprint provenance.
Core also owns mount-table parsing/projection and event-driven observations.
The fixed mountinfo/boot interfaces are pinned and verified as procfs files before
opening them for bounded metadata reads; a replaced leaf cannot invoke an
ordinary device open callback. No generic decoder or payload reader is added.

The policy file now carries source filesystem coordinates, named exclusion owners
and boot/namespace/non-reused mount observations. Client matching uses temporary
O_PATH metadata pins, kernel mount IDs and observed mount roots. It does not retain
media descriptors or rescan mountinfo for each file. An event invalidates the old
observation; new publication refreshes even a renamed mountpoint parent. Metadata
failure, missing provenance and stale snapshots fail closed. Equivalent verified
publications retain the immutable Arc shared with existing cache retirement.

Offline owners retain their known filesystem exclusion while an alias remains
mounted in the same boot/namespace. Old literal paths are not applied to unrelated
replacement media. Removing a rule releases its retained observations, and
returning the original path refreshes its lifetime. FAT/exFAT/NTFS rules are routed
through the current partition views, including a first-time rule on a subtree-only
view. Generic drive routing refuses that subtree as a whole-partition root.
Routing and publication must share a mount signature. Mounted catalog/image paths
are fetched from their indexed mount registries in bounded key pages; a test skips
2,200 invalid historical headers and preserves all 261 mounted paths, including
raw names. The local policy file is included in explicit storage accounting.

The origin-removal regression failed on FAT32 and NTFS before retention was added
(`/tmp/portable-drive-vm-alias-origin-control/report.json`). Its exFAT sequence
stopped at an external FUSE mount-helper restore failure and is not evidence for
that product assertion. Restoration now explicitly uses the native kernel path.
Bare loop volumes remain outside removable inventory; their real mount routing
is tested through the same selector used by production discovery. Earlier attempts
to rename a mounted directory itself failed setup with EBUSY; the final test moves
its ordinary parent instead.

Final disposable LTS acceptance passes **six cases**: two per native FAT32, exFAT
and NTFS3 source. Cases cover whole-directory/subtree/file binds, symlink aliases,
new-mount invalidation, stale and republished renamed-parent views, ordinary-file
allowance, origin unmount/republication, first-time partial-view policy and
subtree-root refusal. Source unmount succeeds with clients retaining their
observations, proving no media descriptor is held. Evidence:
`/tmp/portable-drive-vm-routed-alias-final/report.json` and `guest.log`.

A real Btrfs subvolume control failed before correcting per-object device-number
assumptions (`/tmp/portable-drive-alias-btrfs-before.log`). Kernel Btrfs getattr uses
per-subvolume device numbers, so same-mount continuity now uses the non-reused
mount ID while projection uses the backing mount record. Exact object validation
is unchanged. The owned writable fixture passes private/public admission and
subvolume-boundary checks (`/tmp/portable-drive-alias-btrfs-verified.log`) with VFS
cleanup. The host's unprivileged delete ioctl refusal is not an elevation request;
ordinary removal works and leaves no fixture. See the primary
[Btrfs getattr source](https://raw.githubusercontent.com/torvalds/linux/master/fs/btrfs/inode.c)
and [mount poll source](https://raw.githubusercontent.com/torvalds/linux/master/fs/proc_namespace.c)
for the kernel behavior used by these observations.

Ordinary checks pass **204 backend tests** (84 opt-in ignored) and **466 core tests**
(four opt-in ignored), recorded in `/tmp/portable-drive-alias-routed-verified.log`.
Files policy tests pass 19 cases (`/tmp/portable-drive-alias-files-policy-tests.log`).
The policy doctest and rendered core API documentation pass; generated pages for
IndexPolicy, MountLifetime and Watcher were inspected. Final four-package
all-target Clippy passes with warnings denied
(`/tmp/portable-drive-alias-final-clippy.log`). Private dark storage acceptance
and light policy-editor acceptance pass; the final report/editor screenshots
were inspected (`/tmp/portable-drive-alias-storage_dark/report.json` and
`/tmp/portable-drive-alias-policy_light/report.json`). The former includes the
policy snapshot in apparent registry storage, and the latter preserves the
independent saved flags beyond status pagination.
The final core doctest suite passes all 14 cases and current core API generation
passes (`/tmp/portable-drive-alias-final-doc-tests.log` and
`/tmp/portable-drive-alias-final-rustdoc.log`). Targeted Rust formatting and
`git diff --check` pass.

The final architecture comparison retains 4,300 clone groups and 144 cycles,
average CCN 3.49, maximum 274 and 7.01% duplication. No new hotspot in this alias
implementation is reported. Whole-branch classification remains REGRESSION
against the original baseline (+11 clone groups and +1 cycle); that wider review
remains open. Mount observation/projection and lifetime validation are reused
from core rather than duplicated in policy clients. Policy owners and view
routing are necessary to retain exclusions when their original mount disappears;
they do not introduce another filesystem payload reader or content cache.

Items 1.6, 6.2 and 6.3 remain Partial. Case-fold matching, wider relocation and
consumer coverage, unknown/unsupported-driver behavior and the full interruption
and responsiveness matrices are not proved by these six cases. In particular,
policy metadata observation still participates in the owner's publication path;
unavailable/hung metadata needs its responsiveness gate. The known 7.3.4 exFAT
durability limitation remains separately open; these alias cases use the LTS guest.

## Persistent policy address checkpoint

Rule writes now resolve a discovered device selector to an unambiguous persistent
drive ID before acceptance. Execution uses that binding; it does not resolve the
device node again. A controlled queue test reuses the node for another partition
before delivery and proves the saved policy remains with the original ID. The
same request identity returns the original accepted/completed operation after
reuse. Duplicate persistent identities are refused even when one device node
would otherwise select a single clone. Stable identities remain editable offline.

Relative paths now share canonical lexical keys across repeated separators,
trailing separators and interior dot components. Raw Unix bytes survive. A
versioned, bounded-page local migration merges legacy aliases with logical AND
for each admission flag, preserving all exclusions. An injected failure after
more than one page rolls back every replacement and the migration marker;
retry and a second no-op migration preserve all 1,100 rules. Explicit replacement
can subsequently enable both flags without leaving a hidden old exclusion.
Mismatched saved keys are refused before writes, and invalid parent/NUL addresses
create neither rules nor accepted operations.

Preparation runs outside the cancellation/admission mutex and SQLite write
transaction. Admission then repeats its existing replay and closing checks before
recording the prepared request. A held-inventory test proves an independent
Configure request completes and another connection can take a write transaction
before selector preparation is released. This necessary two-stage admission
raises its measured CCN from 16 to 18; it avoids introducing a second state owner,
lock wrapper or pool. Per-record migration and its transaction boundary are
separate, with neither flagged for excessive complexity. Clone groups remain
4,300 and dependency cycles 144, unchanged from the accounting checkpoint;
whole-branch architecture classification remains REGRESSION, with average CCN
3.49, maximum 274 and duplication 7.01%.

The ordinary backend suite passes **203 tests**, with **82 opt-in tests ignored**
(`/tmp/portable-drive-rule-address-backend-verified.log`). Eleven ordinary policy
coverage tests include the eight new address cases; backend all-target Clippy
passes with warnings denied (`/tmp/portable-drive-rule-address-clippy.log`). The
queued RAM-save cancellation fixture now waits for the owner's actual periodic
inventory gate instead of relying on a Rule request to trigger discovery: Rule
selector preparation intentionally happens before admission. Its original
no-destination-creation and retained cancellation assertions remain and pass.

Private dark/light acceptance seeds policies via actual `clipctl drives policy`
commands with a device selector and `many//`, verifies their stored canonical
persistent address, ages them beyond status pagination, then opens both editors
and saves unchanged flags. Reports:
`/tmp/portable-drive-rule-address-dark/report.json` and
`/tmp/portable-drive-rule-address-light/report.json`. No live desktop service,
privileged host mount or real device payload was used.

Item 6.2 remains Partial: lexical spelling normalization is not filesystem
case-fold, directory-bind or policy relocation coverage. Legacy development
records containing only `/dev/...` have no evidence to safely attach them to a
current partition; they are retained without automatic rebinding, and the full
review/migration experience remains unfinished with the wider alias work.

## Explicit local metadata accounting checkpoint

Jobs → Measure local metadata and `clipctl drives storage` now submit a retained
background operation. Routine status polling and opening Jobs do not measure or
prompt for a backup directory. The report separates baseline names/fingerprints,
sync/RAM inventories, pending paths and registry rows from registered optional
catalog files. Logical row bytes exclude page/index overhead; database/WAL/shared
memory lengths are separate apparent-size observations and must not be added to
their logical components. Missing catalog files are counted explicitly. Search
service databases and backup payloads remain outside this owner's report.

The implementation reuses the retained bulk pool and keeps control requests and
independent synchronization responsive. One read-only SQLite snapshot uses
byte-length aggregates instead of decoding all stored JSON. Keyset pages bound
catalog identifiers; catalog files are only statted, including invalid SQLite
payloads in the test. O_PATH identity pins avoid disturbing SQLite's process-wide
POSIX locks. Replaced state roots and database/working-file aliases are refused.
Cancellation interrupts the SQL VM; the actual 30-second deadline also interrupts
a streamed billion-row query without publishing a partial successful report.

A no-index/no-catalog enrolled pair still copies its complete payload and retains
its required baseline. Measurement succeeds after removing the source from the
inventory and renaming its directory; the dated completed report survives daemon
shutdown. The ordinary backend suite passes **195 tests**, with **82 opt-in tests
ignored** (`/tmp/portable-drive-storage-backend-verified.log`). The explicit real
budget run passes six selected cases including the 30-second timeout
(`/tmp/portable-drive-storage-budget-tests.log`); the later additional alias guard
passes with the six ordinary accounting checks
(`/tmp/portable-drive-storage-guard-tests.log`). Core/CLI/Files drive tests pass
**26 cases**, and all-target Clippy for all four affected packages passes with
warnings denied (`/tmp/portable-drive-storage-host-tests-final.log` and
`/tmp/portable-drive-storage-clippy.log`).

Dark/light private-desktop acceptance exercises explicit Jobs measurement with
both policy flags disabled, verifies the full four-path sync baseline and zero
optional catalogs, then submits and completes a separate actual CLI measurement.
Reports: `/tmp/portable-drive-storage-dark-v5/report.json` and
`/tmp/portable-drive-storage-light-final/report.json`. Screenshots retain the dated
report and separate categories. Setup completion notices are dismissed before
locating Jobs because their status dock moves the sidebar; earlier locator-only
failures are not evidence of product acceptance.

The original-baseline architecture comparison remains REGRESSION: 4,300 clone
groups, 144 cycles, average CCN 3.49, maximum 274, duplication 7.02%. This checkpoint
adds no further clone group or dependency cycle and no flagged production
function in the measurement, worker or dialog paths. Existing branch-wide
architecture findings still need final review. Item 6.2 remains Partial for rule
address aliases and selector handling.

## Mounted-original revision checkpoint

Metadata-only mounted originals now require their captured root and full
observable per-file revision, rather than only size and modification time.
Validation uses the same retained descriptor as FileSource and repeats its
revision check after the fuller comparison. The regression test failed before
the change and passes afterward for changed bytes/ctime, replaced inode,
different captured root, legacy missing details and unknown change time.
Known digests retain their existing content-validation path.

Mounted originals can also reuse a verified matching file-backup manifest's
fingerprint at open time. This uses the existing bounded candidate list,
manifest verification and catalog-mount admission guard; it does not add another
registry scan, retain all candidate roots or alter capture's metadata-only
policy. Real source/FUSE tests pass on FAT32, exFAT and NTFS3: unchanged originals
read normally; a same-size/restored-mtime rewrite falls back to a valid backup;
replacing the original inode invalidates an already-open source handle; absent
backup plus changed identity returns ESTALE; explicit refresh reads new bytes.
The FAT32 fixture actually retains its entire metadata revision after the
rewrite, exercising fingerprint validation instead of assuming ctime changed.

The three provider cases (original revisions, file-backup tampering and nested
catalog-mount refusal) pass on all three formats in
`/tmp/portable-drive-vm-external-write-control/guest.log`. That whole guest report
is **not** green: its separate exFAT durability control fails, as described in
the following checkpoint. Host provider/FUSE acceptance passes all 31 selected
cases (`/tmp/portable-drive-original-provider-fuse-verified.log`), and the
source-payload-read trace control passes
(`/tmp/portable-drive-original-provider-source-trace.log`). Ordinary backend
tests pass 189 cases, with 81 opt-in cases ignored
(`/tmp/portable-drive-provider-and-flush-backend.log`).
All-target backend Clippy passes with warnings denied
(`/tmp/portable-drive-provider-and-flush-clippy-final.log`). The original-baseline
architecture comparison still reports REGRESSION across the branch, with 4,300
clone groups, 144 cycles, average CCN 3.49, maximum 274 and duplication 7.02%.
There is no additional clone group or cycle from this checkpoint.

Item 6.7 remains Partial. Unobservable same-inode changes without a trusted
fingerprint, source remount/revision ambiguity and cross-driver precision still
need work; this checkpoint does not claim exact byte identity from metadata.

## Whole-filesystem pre-unmount flush and isolated exFAT kernel regression

Partition preparation now retains its admitted ordinary source root through the
existing Flush worker and calls guarded Root::flush after enrolled pairs finish.
This covers ordinary application writes even with no enrollment. Failure or
cancellation prevents unmount admission, and the worker releases its root before
the authority stage. No new executor or unmount path was added. The ordinary
189-test backend suite and all-target Clippy with warnings denied pass as
recorded above. An owned unprivileged FUSE fixture also passes external-reader
busy refusal, successful pin retirement, independent RAM writeback during a held
unmount and retained completed cancellation
(`/tmp/portable-drive-pre-unmount-flush-live.log`).

The durability failure has now been reduced outside little-clip. The standalone
`scripts/drive_tools/bind_durability.sh` uses no application executable, FUSE
helper or system bus. A plain directory bind/unmount/removal followed by a new
write, timestamp update, syncfs, native unmount and native read-only remount
reproduces zeros on `7.3.4-1-cachyos` exFAT. Explicit file fsync does not prevent
it. File binds and ordinary create/remove controls pass. All three filesystem
formats pass the identical directory-bind probe with `6.18.60-2-cachyos-lts`.
Fresh-volume application durability controls also pass on 7.3.4. This establishes
a guest-kernel/filesystem regression with a directory-bind trigger, rather than
an image-copy or missing file-fsync explanation. No host kernel was changed.

Evidence: `/tmp/portable-drive-vm-fresh-durability/report.json`,
`/tmp/portable-drive-vm-fsync-durability/report.json`,
`/tmp/portable-drive-vm-durability-prefix-nested/report.json`,
`/tmp/portable-drive-vm-bind-durability-directory/report.json`,
`/tmp/portable-drive-vm-standalone-bind-final-current/report.json`,
`/tmp/portable-drive-vm-standalone-bind-final-current-fsync/report.json`, and
`/tmp/portable-drive-vm-standalone-bind-final-lts/report.json`. The standalone
script's shell syntax and early host refusal pass. The initial standalone run
with an unsupported touch date syntax failed setup and supplies no durability
evidence; the final probe uses the verified POSIX timestamp form.

**Current-kernel negative evidence remains open.** After source-file creation/deletion and
catalog/provider operations, the guest's exFAT source can lose a newly written
file's bytes across syncfs and unmount. The cached file reads the expected
payload immediately before preparation; a kernel remount reads the same length
of zeros afterward. Both the partition-image checksum and a fresh authorized
raw-partition checksum agree, and a native kernel mount of that image also reads
zeros. Therefore a matching raw-image checksum alone does not close this
durability case. Adding the filesystem-wide preflush did not resolve it.

The separate ignored test
`service::tests::native_images::native_external_write_flush_survives_unmount_without_imaging`
reproduces the exFAT failure without the backup copy or a running coordinator:
ordinary write, modification-time restoration, Root::flush, UDisks unmount, then
a read-only kernel remount. FAT32 and NTFS3 pass both the kernel and UDisks
remount controls. The test follows the same three provider tests that expose the
allocation/revision history; this context must be retained when reproducing.
Evidence: `/tmp/portable-drive-vm-external-write-control/report.json` and its
`guest.log`. The strengthened image test has a separate kernel image-mount
control; evidence is `/tmp/portable-drive-vm-pre-unmount-flush/report.json`.
Both reports correctly say failure. Do not replace these with the earlier green
fresh-volume image results or count the exFAT format as passing this sequence.

The standalone probe and cross-kernel comparison now isolate the kernel-specific
trigger. Keep the affected 7.3.4 evidence as an explicit compatibility limitation
and use the separate LTS matrix to assess application behavior. Items 4.3, 5.1,
5.6, 7.3 and the wider interruption/durability gates remain Partial. No host
block device, privileged mount or live desktop service was used.

The complete nine-case bundle per filesystem now passes on the LTS kernel:
four maintenance cases, original revisions, file-backup tampering, nested catalog
mount refusal, direct flush/remount durability and the authorized image lifecycle
(27 selected cases; `/tmp/portable-drive-vm-lts-drive-acceptance-final/report.json`).
The initial LTS bundle failed a timing assumption in the startup-maintenance
fixture: enrollment consumed the remaining cleanup interval. The fixture now
enrolls the independent pair before restart, preserving its strict live-work
checks during Configure, sync, shutdown and restart. All four maintenance cases
also pass on the host in 25.61 seconds
(`/tmp/portable-drive-maintenance-preenrolled-host.log`). This test correction
does not change the maintenance implementation or weaken its assertions.
The original-baseline architecture comparison remains REGRESSION with 4,300
clone groups, 144 cycles, average CCN 3.49, maximum 274 and duplication 7.02%;
this follow-up adds no clone group or cycle.

## Retained maintenance checkpoint

Follow-up native acceptance passes all nine selected maintenance/provider/image
cases per FAT32, exFAT and NTFS3 partition (27 cases total). The two 10,000-file
responsiveness cases exercise explicit cancel/retry and automatic shutdown/restart
on each source format. The guest report is
`/tmp/portable-drive-vm-maintenance/report.json`, with test results in its
`guest.log`. exFAT's userspace image provider still loses fractional modification
time and correctly returns ESTALE; this does not close the precision gate.

Legacy destination retirement now atomically retains its root policy enrollment
before dropping the final cleanup journal. New publication also uses this same
registry writer. A fault-injection test removes all prior root enrollments,
uses an invalid-UTF-8/control-character path, withdraws the old location's last
version with its source offline, and fails the root-registry insert after data
and manifest removal. The journal retains the exclusion until retry succeeds.
Two subsequent daemon starts, with no backup preference, retain the exclusion
and reject sync enrollment into the old location. The ordinary backend suite
passes 188 tests (79 opt-in cases ignored), and all-target Clippy passes with
warnings denied. Evidence: `/tmp/portable-drive-legacy-root-tests.log`,
`/tmp/portable-drive-legacy-root-backend.log`, and
`/tmp/portable-drive-legacy-root-clippy.log`.
The updated four-case maintenance bundle, native versioned-file backup and
authorized image lifecycle also pass on each FAT32/exFAT/NTFS3 partition
(18 selected cases; `/tmp/portable-drive-vm-legacy-roots/report.json`). The
architecture comparison retains 4,300 clone groups, 144 cycles, average CCN
3.48 and maximum 274. A repeated policy assertion was consolidated in the test
fixture; retirement finalization's CCN 16 includes the necessary checked parent
derivation and atomic root enrollment. No new clone group or cycle remains
from this follow-up.

Explicit pruning and startup backup recovery now share the existing retained
bulk pool with catalog maintenance. The owner schedules bounded rowid passes;
workers perform manifest checks, publication and tree retirement. Recovery
identifiers and pending publication decoding are bounded. Cancellation reaches
manifest hashing and retirement. User cancellation persists a paused retirement;
shutdown cancellation leaves automatic recovery resumable. Prepared and retiring
legacy destinations remain excluded during their journal lifetime. A recovered
older publication cannot roll the current registry or filesystem reference back
over a newer completed version, including with misleading wall-clock dates.

Host acceptance uses 10,000-file withdrawn versions with the source offline.
Both explicit pruning and startup recovery keep Configure and an independent
pair's sync responsive while retirement remains active. The explicit case cancels
and retries; the startup case shuts down and restarts before completion. All three
maintenance tests pass (28.50 seconds, 18,596 KiB peak child RSS). The ordinary
backend suite passes 187 tests, with 79 opt-in cases ignored; all-target backend
Clippy passes with warnings denied. Unprivileged real Btrfs retention also passes
after these changes. Evidence: `/tmp/portable-drive-maintenance-live.log`,
`/tmp/portable-drive-maintenance-backend-final.log`,
`/tmp/portable-drive-maintenance-clippy-final.log`, and
`/tmp/portable-drive-maintenance-btrfs.log`.

The original architecture-baseline comparison still reports REGRESSION across
the full branch: 4,300 clone groups and 144 cycles, unchanged from the preceding
retention checkpoint. Average CCN is 3.48, maximum 274 and duplication 7.03%.
Publication installation drops from CCN 51 to 39 by separating artifact
installation from current-reference/registry publication. The remaining
branch-wide findings still require final review.

Items 1.5 and 4.6 remain Partial: source/destination pin admission, remaining
mount controls, slower storage, native-format responsiveness and wider fault
matrices need further evidence. Legacy roots remain protected after final journal
removal; bind/case-fold/relocation aliases remain in the wider no-index work.

## Durable backup retirement checkpoint

Pruning now withdraws a selected completed version and inserts its versioned local
backup-prune intent atomically before any filesystem mutation. Selection preserves
the existing per-kind keep count, current pointers and descending registration
order when wall-clock dates tie. Parent write/search/read-only admission happens
before withdrawal. Current/publishing/mounted versions and changed completed
records are refused again inside the transaction. Raw current-pointer protection
works even when a completed record is missing.

Cleanup retains exclusive artifact ownership through the initial attempt. A
no-overwrite rename moves the artifact to .pruning-{UUID} in its identity-checked
parent. The version-1 JSON intent retains the Backup, parent StorageIdentity,
quarantine observation and paused state. File/directory identities are required
on every recovery lookup, and unknown versions/layouts are refused. Manifest
checksum/header validation precedes payload deletion; metadata retirement uses
the existing guarded checksummed deletion. Final parent fsync precedes atomic
journal/recovery-record removal. Withdrawn versions cannot appear as complete
backup providers or incremental bases.

The existing publication recovery entrypoint also visits retiring versions in
bounded key pages, with 128-byte identifier and 64-KiB JSON decode limits. Failed
attempts retain DriveRecovery details. Cancellation persists paused=true and
startup leaves it paused; another explicit prune request resumes it. Successful
per-version removals remain counted if a later candidate fails. Retirement uses a
bounded stack of the existing guarded scan frames rather than leaving a second
full tree index in operational state. Traversal refuses foreign mounts before
opening them and never follows a symlink payload. Owner directory permissions can
be repaired only inside the withdrawn tree.

Btrfs retirement first uses the ordinary subvolume deletion command. On this
host's mount it returns EPERM without user_subvol_rm_allowed; the fallback clears
the retired owned root's read-only property, then performs descriptor-relative
VFS removal. The same retained-handle property setter seals new backups, replacing
duplicated command construction. The current version stays sealed and untouched.
No daemon elevation, mount-option change, privileged host mount or source-drive
metadata file is introduced. Btrfs subvolume roots may have another st_dev without
a mount crossing: admission retains the existing NO_XDEV child pin and requires
the exact saved StorageIdentity instead of weakening ordinary recursive traversal.

Six ordinary tests cover atomic withdrawal faults, parent permission refusal,
raw current-pointer protection, cancellation/restart/explicit retry, six replaced
or corrupt input cases, and six abrupt-exit subprocess boundaries. The child exits
with the WAL connection and relevant pins/leases live, without Rust cleanup.
Boundaries include withdrawal, a lost-quarantine-ack control, quarantine, partial
deletion, absent artifact and absent manifest. A reopened store recovers twice
without advertising partial data or damaging current bytes. The simulated
filesystem boundaries do not replace the still-open live-device interruption and
full daemon responsiveness matrix.

Final ordinary backend results: 183 pass, 77 explicit fixtures ignored. The fresh
helper passes all 30 host provider cases, including the missing-status open-reader
guard. Backend all-target clippy with warnings denied, targeted formatting and
whitespace checks pass. The six-child source-read trace still passes. Logs use
`/tmp/portable-drive-retention-journal-` with backend/fuse/trace/clippy final logs.

The real unprivileged Btrfs acceptance prunes two sealed older snapshots while
preserving current bytes and an outside symlink target. It exercises raw-named
nested files and directory permissions. Its write control returns EROFS before pruning.
The syscall trace proves both deletion ioctls return EPERM, both retired roots are
unsealed, and guarded rmdir removes only those roots. Evidence:
`/tmp/portable-drive-retention-journal-btrfs-final.log` and its `.trace` companion.
The owned Btrfs fixture is removed without elevation. A first test incorrectly
equated statvfs mount flags with subvolume read-only state; the actual write control
replaces that assumption. Stable equal-date selection was also corrected after a
test caught reversed registration order in the new visitor-based plan.

`/tmp/portable-drive-vm-retention-journal/report.json` records successful FAT32,
exFAT and NTFS profiles, no timeout, QEMU exit 0 and one LITTLE_CLIP_VM_RESULT=0.
Each profile runs six selected native cases. The authorized partition-image case
now explicitly configures retention, prunes its older image through the service,
verifies its manifest is gone and hashes the retained current image exactly.
The other cases retain file-backup/catalog inclusion, provider, recursion and
unsupported fingerprint-promotion checks. Known exFAT timestamp loss remains
explicitly stale.

Architecture: 4300 clone groups and 144 cycles, unchanged from the preceding
reader checkpoint; duplication 7.03%, average CCN 3.49 and maximum 274. Initial
clone reports were removed by sharing the subvolume property mechanism, bounded
entity reader and overlapping crash fixtures. Cohesive admission, withdrawal,
artifact cleanup and finalization phases eliminate the new CCN>15 reports.
Structural analysis still flags artifact open, withdrawal and finalization as
central resource/transaction boundaries; those required joins remain for the
final branch review. Necessary journal/recovery work adds complexity, but no new
clone or cycle is accepted by this checkpoint. The original-baseline branch result
remains REGRESSION. Filtered report:
`.fitness/portable-drives-retirement-journal-architecture.json`.

Items 4.6/6.8 remain Partial for the wider provider/prune/device interruption and
retention responsiveness gates. The complete goal remains active.

## Backup retention reader checkpoint

Two negative tests reproduced deletion of a backup supplying an open catalog
descriptor and missed mounted-reader status beyond 1,000 records. Both are fixed.
Evidence: `/tmp/portable-drive-retention-readers-negative.log`.

File-backup providers acquire nonblocking shared locks on an independently opened,
guarded root-directory descriptor and retain that descriptor until OpenFile drops.
Pruning needs the matching exclusive lock before touching the tree. Both direct
readers and the separate FUSE helper remain protected independently of status
publication. Contention returns a busy result rather than waiting on the server
or coordinator. Identity, revision and content validation remain required after
admission; the lease does not substitute for them.

Mounted-catalog admission now visits a transactionally fixed set of all catalog
records through the existing bounded entity visitor. It checks both published
mounts and actual catalog mount sources, protecting older helpers without the
new per-provider lease. It does not use a status-page cut-off. The new separate
FUSE test removes the catalog status record while retaining its open file, then
proves that pruning is refused, the original bytes remain readable, and pruning
succeeds after release and helper exit.

Final backend tests pass 177 cases (75 explicit fixtures ignored); the fresh
helper passes all 30 selected host provider cases. Backend all-target clippy with
warnings denied, targeted formatting and whitespace checks pass. Logs:
`/tmp/portable-drive-retention-readers-{backend,fuse}-final.log` and
`/tmp/portable-drive-retention-readers-clippy.log`.

Architecture metrics retain 4300 clone groups, 144 cycles, 7.04% duplication,
average CCN 3.48 and maximum 274. A shared captured-file fixture removes the
initially reported test clone. New admission/lease functions have no CCN hotspot;
the graph analysis flags the unchanged provider backup_entry function for final
branch review. The original-baseline result remains REGRESSION. Filtered report:
`.fitness/portable-drives-retention-readers-architecture.json`.

Items 4.6/6.8 remain Partial. Durable retirement, Btrfs deletion capability and
interrupted cleanup still need implementation and acceptance. This reader guard
does not claim those gates or retention responsiveness are complete.

## Metadata-only catalog file-provider checkpoint

Two new negative tests reproduced wrong-byte acceptance: a completed file backup
could be altered while preserving size/mtime, and a newer backup could substitute
a changed source file with matching size/mtime. Both catalog entries lacked a
captured digest. Evidence: `/tmp/portable-drive-provider-manifests-negative.log`.

File providers now retain the completed Backup record through admission. Without
a catalog digest, they require its checksummed completed manifest, validate the
manifest header and captured source-root device/inode against the catalog, and
match the full recorded source file revision and raw path. They then hash the
retained provider descriptor against the manifest's SHA-256 before returning any
contents. Existing captured digests can still authorize exact bytes without
optional manifest enrichment, while changed timestamps remain stale. Missing
legacy source identities or manifests require refresh or an existing captured
digest. A stale newest version can fall back to a verified older match; open
handles retain their original provider and detect subsequent changes.

The completed manifest locator is shared with history enrichment. Point inventory
lookups now limit JSON records to 64 KiB before decoding, and optional sync hash
imports reuse that reader instead of owning another SQL/decoder path. Normal
inventory rows contain bounded metadata rather than payloads; malformed oversized
records return an error rather than allocating arbitrary JSON strings. No schema
or removable-media metadata files are added.

Manifest reads also use a held ordinary parent and the existing descriptor-relative
NO_XDEV input path. A manifest bind-mounted to the serving catalog must be refused
before a foreign FUSE open, preventing recursion in its single-threaded server.
The same guard applies to publication verification and optional hash enrichment.
Checksum/header admission and caller queries remain bound to one retained file,
with final file and parent validation. An explicit negative VM using the preceding
helper hung in the expanded recursive-provider test and hit its 90-second guest
deadline: `/tmp/portable-drive-vm-provider-manifests-negative-shared/report.json`.
An earlier harness attempt named an unshared host /tmp helper and failed to launch;
that attempt is not recursion evidence.

Five ordinary revision tests cover tampering, source changes, raw names, fifteen
manifest/source-record faults, known-digest compatibility and older-version
fallback. The new real FUSE case verifies ESTALE on altered bytes, an already open
handle and a corrupt manifest while listing/stat remain available. The native
case runs both direct and actual FUSE acceptance on each source filesystem.

Final ordinary backend results: 175 pass, 74 explicit fixtures ignored. The fresh
helper passes all 27 selected host provider cases. Final all-target backend clippy
with warnings denied, targeted formatting and whitespace checks pass. The existing
six-child strace acceptance still verifies no source payload reads for current
backup, historical backup and sync enrichment, with deliberate read controls.
Logs: `/tmp/portable-drive-provider-manifests-{backend,fuse,trace,clippy}-final.log`.

`/tmp/portable-drive-vm-provider-manifests/report.json` records success without
timeout, QEMU exit 0 and exactly LITTLE_CLIP_VM_RESULT=0. Each real FAT32/exFAT/NTFS
profile passes six selected cases: new metadata-only backup/FUSE validation,
recursive payload/manifest refusal, unsupported hash-promotion refusal, versioned
file backups, exclusion versus full backup/sync inclusion, and authorized image
backup/read-only providers with the denial-child control. Recursive-provider
refusal finishes in about 0.12 seconds per profile. Existing exFAT image timestamp
precision loss remains explicitly stale.

A real Btrfs read-only file backup also supplies multi-megabyte catalog reads
through the actual helper and preserves all 22 FUSE read-only mutation refusals.
Evidence: `/tmp/portable-drive-provider-manifests-btrfs.log`. The read test passed;
the outer cleanup initially returned EPERM from the subvolume deletion ioctl on
this host mount without user_subvol_rm_allowed. Only the owned fixture's read-only
property was then cleared, and ordinary directory removal deleted the complete
owned tree without elevation. No fixture or mount remains. This reinforces the
still-open Btrfs retention/capability gate rather than claiming cleanup acceptance.

Architecture comparison retains 4300 clone groups, 144 cycles, duplication 7.04%,
average CCN 3.48 and maximum 274. Sharing native fixture setup removed the initial
new clone. Separating checksum/header admission from retained-path/callback
lifetime removes the guarded reader's initial CCN=17 hotspot; neither phase nor
the new provider functions has a final reported hotspot. Remaining scope flags
predate this checkpoint. The branch result remains REGRESSION against the original
baseline. Filtered report: `.fitness/portable-drives-provider-manifests-architecture.json`.

This checkpoint does not make coarse source metadata
proof of byte equality, fix original-provider uncertainty, round mismatched image
timestamps or close the provider lifetime/concurrency matrix. Per-open manifest
verification streams its full checksum input; capture remains metadata-only.
Items 6.7/6.8 and the branch-wide completion goal remain Partial/active.

## Historical backup fingerprint checkpoint

Capture now tries the explicit current completed file backup first, then completed
history in reverse registration order. Matching drive/evidence, kind, identifier,
manifest location, checksum/header, queried-change provenance, source-root identity
and full per-file revision remain required. It does not select prepared publications
or change current-version pointers. Each candidate has an independent transaction:
failure cannot discard earlier imports, and older candidates cannot replace a
verified current hash. Raw file names remain lossless.

A single verification allowance starts at the effective catalog byte budget. The
existing guarded manifest reader reserves the exact FileSource-observed length
before hashing, even if verification fails. Oversized inputs are refused before
reading; cancellation/zero deadlines detected before admission leave the allowance
untouched. The new core observed_size accessor returns that captured length rather
than refreshing it from a potentially changed file. Existing FileSource checks and
bounded positional hashing remain authoritative. It does not change revision-key
representation. Header/source/provenance records use the shared bounded entity
reader; backup records/rows are limited to 64 KiB before decoding.

An additive kind/rowid index supports history without a growing in-memory list,
a full-history sort or status-page cut-off. Startup upgrades existing state in
place without changing database versions. Standalone pre-index callers retain
current-version enrichment; daemon startup enables history. Verified manifests
stream their rows and query captured paths through the existing index. A missing
eligible-file count is computed once, imports decrement it only after commit,
and completed coverage stops further historical verification. Empty/no-candidate
catalogs avoid that count. Time and cancellation checks span the complete search.

Seven focused history cases cover twelve current-candidate faults, explicit old
current pointers and changed clock values, independent rollback/gap filling,
aggregate allowance refusal and larger-limit recovery, over 1,100 newer unrelated
records, old-index migration and raw file names. They retain older manifest bytes
unchanged. The trace acceptance exercises current backup, sync and historical
fallback, each with no-read and deliberate source-read controls. It also verifies
that an already-covered older manifest is not read.

Evidence: `/tmp/portable-drive-history-hashes-backend-final.log` records the final
ordinary backend run (170 pass, 72 ignored); the explicit FUSE, trace and native
fixtures below were run separately. Final backend all-target clippy passes with
warnings denied (`/tmp/portable-drive-history-hashes-clippy-final.log`).
Nine core guarded file-source tests pass, including unchanged observed length
after source growth; all twelve core doctests pass. The rendered observed_size
summary and its resolved check_file link were inspected. Logs use
`/tmp/portable-drive-history-hashes-`. The fresh helper passes 21 host FUSE provider
cases. `/tmp/portable-drive-vm-history-hashes/report.json` passes FAT32, exFAT and
NTFS: each profile checks unsupported promotion/reuse refusal, ordinary versioned
backups, catalog policy versus full backup/sync inclusion, and real authorized
imaging/read-only providers with explicit denial-child verification. The separately
tracked exFAT fractional-time limitation remains unchanged. No host mount, attached
medium or live desktop daemon was used.

Architecture comparison retains 4300 clone groups, 144 cycles, average CCN 3.48
and maximum 274; duplication is 7.04% versus the original baseline's 7.30%.
The branch-wide result is still REGRESSION. New history/resource functions have
no reported CCN hotspot; the structural tool flags catalog/hashes.rs:merge for
the final architecture review. Bounded decoding is shared with sync provenance.
The existing publication install/seal and Store.visit flags and short scan/store
clone predate this checkpoint. Filtered checkpoint report with full metric deltas:
`.fitness/portable-drives-history-hashes-architecture.json`.
Item 6.7 stays Partial for uncertain provider revisions and cross-driver precision;
no broader feature gate is marked complete by this checkpoint.

## Sync fingerprint enrichment checkpoint

Portable full-scan caches now publish versioned source provenance atomically with
their scan rows. It includes drive/evidence, lossless subtree, filesystem/inode/birth
identity, boot, complete namespace, unique mount ID and a generated row cohort.
Only queried-change-capable local sources with a known birth identity qualify.
The existing scan writer tags freshly verified portable rows; it does not run a
second tree pass to add markers. Named portable observations update their rows
under the same proven context. An incoming copy does not inherit the destination's
old fingerprint. A later fresh observation can make that file eligible.

Catalog enrichment reads one registry snapshot, visits captured entries once and
finds candidates through ancestor-prefix lookups and existing indexed scan/path
queries. It checks full revision, canonical relative row path and matching row
cohort, with at most 512 enrollment candidates, 16 KiB enrollment/provenance rows
and 64 KiB fingerprint rows. Cancellation and the original catalog time/byte
budgets remain in force. Optional failure rolls back sync enrichment while retaining
ordinary metadata and any prior verified backup enrichment. Existing catalog,
manifest and ordinary state versions stay unchanged; absent cache-row markers
serialize exactly as before. Markers and provenance live only in operational state.

Legacy/mismatched generations cannot be relabeled: poisoned-digest controls force
fresh hashing before a new provenance record can be published. Faults in full-scan
or named-row publication retain completed file effects, the old scan/provenance
and common baselines; a clean retry performs no needless copy. Missing/future or
changed provenance, raw subtree names, changed same-size/mtime revisions, oversized
rows/registries, late malformed rows, backup digest priority and named incoming
copies have targeted coverage.

Review also found two existing reuse-admission gaps. Named directory/root events
could fall back to full scans with reuse enabled despite unknown observation
coverage. Direct full-scan calls could request reuse on coarse/unknown filesystems.
Fallbacks now take fresh scans, and execution checks each root's cache capability.
Owned poisoned-cache controls prove these checks rather than relying on timing.

Shared boot/namespace/mount continuity is extracted from the image provider into
one backend owner (`ebe4e8cb`). Image observations retain their exact flat JSON
fields, verified by a legacy representation fixture and real image-provider tests.
Boot identity remains a fixed, bounded, non-consuming procfs read with filesystem
and no-follow/nonblocking checks.

Final evidence: `/tmp/portable-drive-sync-hashes-backend-verified.log` passes 163
ordinary backend cases (72 explicit fixtures ignored). The 21 host FUSE provider
cases pass in `/tmp/portable-drive-sync-hashes-providers.log`. The explicit strace
case passes for backup and sync capture, with negative/no-read and positive/read
controls for both; traces are `/tmp/portable-drive-catalog-hash-source-trace-{backup,sync}-{false,true}.log`.
All-target backend clippy with warnings denied and changed-source rustfmt/whitespace
checks pass. The final guest report
`/tmp/portable-drive-vm-sync-hash-providers-verified/report.json` passes all three
native source profiles, each with coarse-source/generation refusal, union/conflicts,
deletion policy, catalog exclusion versus full sync/backup inclusion, and real
authorized imaging/read-only provider lifetime checks. The image parent requires
its explicit unprivileged denial child marker as before. The known exFAT FUSE
fractional-time limitation is unchanged.

The earlier combined guest report is intentionally retained as unsuccessful:
imaging changed the source mount path before later fixed-path fixtures ran, whose
filesystem guard correctly refused the exposed tmpfs directory. The final command
runs fixed-path cases first and imaging last. No host mount, attached medium or
live desktop daemon was used.

Architecture still reports REGRESSION against the original baseline: 4300 clone
groups versus 4289, 144 cycles versus 143, average CCN 3.48 versus 3.44, max 274
unchanged and duplication 7.05% versus 7.30%. Clone/cycle counts are unchanged
from `3a4981b1`, with no new scope hotspot or clone after sharing metadata fallback
assertions and separating candidate selection from snapshot orchestration. Existing
scan/reconciliation functions gain the required generation/capability branches;
those guards bind the new reuse behavior, not parallel algorithms. Report:
`.fitness/portable-drives-sync-hashes-architecture.json`. Older manifest enrichment,
uncertain provider revisions, cross-driver precision and the broader feature gates
remain open; item 6.7 is still Partial.

## Configurable catalog worker checkpoint

`drives.catalog_workers` now defaults to one and accepts 1..4 concurrent captures.
The coordinator retains a bounded map of accepted tasks using the existing bulk
executor, retaining each task through delivery and shutdown. Catalogs share the
registry but claim only their independent source for scheduling; captures of the
same partition or overlapping source serialize. Backups remain exclusive with
captures, and metadata maintenance waits for all active captures. Lowering the
setting changes future admission without cancelling accepted work. Status now
publishes accepted settings before advertising Configure completion, independently
of potentially slow subsequent inventory refresh.

The owned host fixture passes default admission, live increase/decrease, four
simultaneous captures, repeated/same-root aliases, an independent queued backup,
and concurrent shutdown/restart with no source inventory. A separate held-inventory
test verifies the completion/settings ordering. The full ordinary backend suite
passes 152 tests (71 explicit fixtures ignored); nine core drive tests and fourteen
Files drive tests pass. Dark and light UI runs each pass 22 observations with a
custom count of three displayed and all per-run budgets preserved.

Final native acceptance passes on FAT32, exFAT and NTFS in
`/tmp/portable-drive-vm-catalog-workers-bounded/report.json`: each profile runs
four worker cases, two per-run limit cases and ten capture lifecycle cases. The
40,000-file fixture uses small directories and empty ordinary files, keeping the
metadata workload within the guest's exFAT allocation budget. The first flat
fixture timed out and exposed the configuration-status race; the first grouped
fixture ran out of exFAT space before capture admission. Those unsuccessful runs
remain recorded under `/tmp/portable-drive-vm-catalog-workers` and
`/tmp/portable-drive-vm-catalog-workers-verified`; neither counts as passing evidence.

The final owned host worker acceptance passes four cases in
`/tmp/portable-drive-catalog-workers-host-bounded.log`. Broader ordinary backend
results are `/tmp/portable-drive-catalog-workers-backend.log`; core/Files and
all-target clippy logs use `/tmp/portable-drive-catalog-workers-`. Dark/light
reports are `/tmp/portable-drive-catalog-workers-{dark,light}/report.json`.
Both UI runs show the custom count of three and preserve configuration through
review cancellation, reset, limited capture and explicit larger capture. No
catalog is accidentally mounted. Final test-only fixture adjustment is covered
by the passing native/host cases and backend all-target clippy. Changed-source
rustfmt, Python compilation and diff whitespace checks pass.

The original-baseline architecture result remains REGRESSION with 4300 clone
groups, 144 cycles, average CCN 3.48, maximum 274 and duplication 7.05%. None of
those counts worsen versus the preceding committed checkpoint. Resource pinning
now has its own admission phase instead of a nested closure in the scheduling
method; no touched new hotspot or clone remains. The existing executor, retained
write-task delivery, scopes and cleanup scheduler are reused with no dependency
addition. Report: `.fitness/portable-drives-catalog-workers-architecture.json`.
Other identity/provider/no-index, responsiveness and final feature gates remain open.

## Catalog capture lifecycle checkpoint

Captures now write inside a private, identity-bound staging directory only after
committing an ownership journal. Cancellation retires the owned SQLite database
and sidecars. A crash leaves a durable intent for the existing bounded maintenance
worker: unfinished captures are retired; closed, fsynced and checksummed captures
are installed without overwrite and advertised transactionally. Registry publication
also queues exclusions added after capture began and clears the ownership intent.
Published catalogs, including subsequently redacted metadata, are never replaced
by an older recovery snapshot.

Ten lifecycle cases pass on host and each native FAT32/exFAT/NTFS source. They cover
cancellation after a committed 128-entry batch, a real child exit after that batch,
a separate exit after sealing a valid cloned metadata snapshot, failed advertisement
with live-daemon recovery while the source is offline, replaced parent/stage/file
identities, unexpected staging entries, same-size changed bytes, extended database
size, late exclusions and stale intents after redaction. Unknown/replaced artifacts
are retained with recovery status. Only empty allocations can precede the initial
ownership commit; this does not claim cleanup of arbitrary legacy/unbound files.

Evidence: `/tmp/portable-drive-catalog-lifecycle-negative.log` reproduces the old
unpublished-database leak. The final backend run passes 150 tests (69 explicitly
ignored); `/tmp/portable-drive-vm-catalog-lifecycle-verified/report.json` passes all
three native profiles, each with ten lifecycle and two limit cases. Final targeted
lifecycle tests pass again after clippy cleanup. The freshly built FUSE helper passes
all 21 host provider cases. All-target backend clippy with warnings denied,
changed-source formatting and diff whitespace checks pass. Logs use the prefix
`/tmp/portable-drive-catalog-lifecycle-`.

The original-baseline architecture result remains REGRESSION: 4300 clone groups
versus 4289, 144 cycles versus 143, average CCN 3.48 versus 3.44, maximum 274
unchanged, duplication 7.05% versus 7.30%. This checkpoint introduces no additional
reported clone/hotspot compared with its preceding committed checkpoint. Shared
recovery classification, maintenance scheduling and redaction assertions avoid
parallel mechanisms. Report: `.fitness/portable-drives-catalog-lifecycle-architecture.json`.
Configurable capture workers were unfinished at that checkpoint and are covered above. Broader feature-wide acceptance remains unfinished.

## Explicit catalog limits for one capture

Save / update catalog now opens a review with entry, database-byte, depth and
time controls, larger-run and configured-limit buttons, partial-coverage disclosure
and the existing single bulk-worker policy. Review cancellation submits no work.
Starting a reviewed capture sends all four chosen values, so a later global
configuration change cannot silently change that selection. CLI flags can
override individual fields; omitted fields inherit the worker's captured custom
daemon settings. The effective budgets are stored with both registry and catalog
header metadata and displayed in catalog descriptions. Older catalogs retain
unknown budgets rather than invented defaults.

The new CatalogWithLimits request is distinct from the unchanged legacy Catalog
request. A legacy protocol fixture proves old daemons reject explicit limits;
ordinary catalog requests retain byte-identical serialized fingerprints.
Override decoding rejects unknown field names. Shared core validation refuses
unusable byte/depth/time budgets before any catalog directory or database is
created. The existing whole-service configuration validator uses that same
validation. Per-run changes never write global configuration, backup preferences
or removable media.

Eight core drive tests, ten CLI tests and fourteen Files drive tests pass. The
two new live-Service tests prove that a configured five-entry capture remains
partial, an explicit larger run completes while inheriting custom byte/depth/time
settings, limits survive catalog database reopen, and the following ordinary
capture retains its original cap. Four invalid override cases leave failed
operations, no catalog database/directory and unchanged preferences. The tests
pass on host storage and each real FAT32/exFAT/NTFS source in the disposable
guest: `/tmp/portable-drive-vm-catalog-limits/report.json`, with success=true,
no timeout, one successful guest marker and three two-test passing groups.

Private compositor acceptance in dark and light appearances verifies review,
expansion followed by Cancel, expansion followed by resetting to configured
limits and a partial capture, and a larger complete capture without changing
defaults. Reports: `/tmp/portable-drive-catalog-limits-dark-final/report.json`
and `/tmp/portable-drive-catalog-limits-light-final/report.json`. Raw rendered
review screenshots are retained alongside the reports; both pass all 22
observations. The saved databases independently confirm a two-entry partial
capture and three-entry complete capture with the selected, distinct budgets.
The harness reuses its
verified Cancel-button interior instead of matching the word "cancel" in body
text, and dismisses the configuration completion notice before locating the
sidebar row. The update token distinguishes review opening from the separately
listed saved-catalog mount action; no catalog may become mounted during this
scenario. Bounded retries open only the review, before the separately verified
Start action. Early missing-OCR-model and wrong-target harness runs are retained
separately; final runs use the existing `/tmp/little-clip-tessdata`.

All 140 ordinary backend tests pass (68 explicit fixtures ignored), as do
all-target clippy for core, CLI, Files and drives under denied warnings, targeted
Rust formatting, Python compilation and whitespace checks. Logs use
`/tmp/portable-drive-catalog-limits-` with `all-build.log`, `ui-build.log`,
`core.log`, `cli.log`, `ui-unit.log`, `host.log`, `backend.log`, `vm.log`,
`clippy.log` and the appearance-specific logs.

Architecture remains REGRESSION against the original baseline: 4300 clones,
144 cycles, average CCN 3.48, maximum 274 and duplication 7.06%. No new clone
group or dependency cycle is reported. Capture adds one necessary validation
guard before database creation (CCN 15 to 16); the existing branch-wide
architecture cleanup remains open. Report:
`.fitness/portable-drives-catalog-limits-architecture.json`.
This completes the explicit larger-run flow, not all of 6.5: configurable worker
limits and unpublished-capture cancellation/restart cleanup still need work.

## Periodic recovery from unavailable watches

Watch registration failures now retain incomplete-coverage state after the
one-shot full-comparison request has been consumed. The configured periodic
boundary requests another incremental rebuild, so restored kernel watch quota
can repair previously missed descendants. Ordinary intervening polls do not
restart the traversal. Unknown/incomplete coverage disables full-scan fingerprint
reuse. Fresh comparisons of specifically notified paths remain available within
validated retained roots, independently from reuse eligibility.

An unavailable inotify instance previously made every 200 ms coordinator tick
request another complete comparison. The coordinator now takes the configured
periodic fallback and retries instance creation at that boundary. Recovery's full
flag invalidates older cached roots and their epoch. Explicit synchronization
still runs without inotify. RAM session/backing/lifetime validation is shared
through the existing RAM session owner, and the symmetric prior/current root
checks use one common comparison for both sides. No formats, public protocols,
configuration defaults or removable-device contents are added by this change.

The guest watch-quota fixture limits the kernel to one watch. Its root registers,
the nested descendants fail, coverage remains incomplete and intervening polls
neither claim complete coverage nor retry. After quota is restored, the periodic
boundary rebuilds all three watches and later nested writes produce events.
The live Service fixture sets the instance limit to zero before startup, proves
that explicit initial synchronization still copies data, and bounds owner
inventory calls while idle to reject continuous complete comparisons. Restoring
capacity and advancing the configured periodic boundary creates an actual
inotify descriptor, verified through this process's descriptor metadata. A fresh
comparison and restored one-hour interval are followed by event-driven copying;
shutdown releases the descriptor. Only the guarded disposable guest changes
kernel limits.

Accepted report: `/tmp/portable-drive-vm-watch-retry-accepted/report.json`. It
runs six watcher tests, both live event-loss/retry tests and both RAM population
recovery tests on each real FAT32/exFAT/NTFS source. The report must have
success=true, no timeout, one successful guest marker and all nine expected
passing test groups. All 138 ordinary backend tests pass (68 explicit fixtures
ignored), with drive all-target clippy under denied warnings, targeted Rust
formatting and whitespace checks. Logs use `/tmp/portable-drive-watch-retry-`
with `accepted-build.log`, `accepted.log`, `backend-accepted.log`, `clippy.log`
and `fmt.log`.

The original-baseline architecture result remains REGRESSION: 4300 clone groups,
144 cycles, average CCN 3.48, maximum 274 and duplication 7.07%. Cohesive RAM
admission validation and symmetric cache checks reduce the observed
prepare_sync_pair CCN from 27 during this step to 15, without omitting a guard.
There are no additional reported touched hotspots, clone groups or cycles.
Report: `.fitness/portable-drives-watch-retry-architecture.json`.
Configured traversal-budget/permission failures, active-worker event loss and
mount-inventory loss remain separately unfinished.

## Rebuilding directory watches after kernel queue overflow

A lost directory-creation event previously left its subtree permanently unwatched,
even after the full comparison repaired its data. A disposable-guest negative
test reproduces missing later nested-file notifications on FAT32, exFAT and NTFS:
`/tmp/portable-drive-vm-watch-overflow-negative/report.json` is expected failure
evidence. A cfg(test) observation flag proves the actual event parser consumed
IN_Q_OVERFLOW, including its unknown watch descriptor; the test also verifies
that the directory-creation events themselves were dropped.

The watcher now replaces its inotify instance after overflow and queues every
retained root for incremental re-registration and traversal. Old watches retire
before their kernel watch quota is reused. Existing five-millisecond/4096-step
registration work and watch/path/depth budgets remain in force. Roots still
waiting to register survive another rebuild, and scope retirement releases their
pins. Rebuilding does no filesystem traversal synchronously. If new-instance
creation fails, the old owner remains live, coverage stays incomplete and a
later tick retries. The daemon's existing full-coverage loss path clears cached
root observations and advances its epoch, preventing an older worker completion
from re-establishing them.

The [inotify manual](https://man7.org/linux/man-pages/man7/inotify.7.html)
documents the per-instance queue limit, dropped events, overflow notification and
the need to rebuild after lost coverage. Only the owned guest changes that limit;
no host sysctl, attached media, desktop or live user daemon is touched.

The accepted direct report is
`/tmp/portable-drive-vm-watch-overflow-accepted/report.json`: success=true, no
timeout, one successful guest marker and five passing watcher tests on each
native source filesystem. Two real overflow cycles per format create missed
subtrees beneath the root and an existing child. Once registration settles,
later writes in both produce ordinary path notifications without a full scan.
An ordinary host test additionally interrupts rebuilding before registration,
rebuilds again, retires one queued scope and verifies the other still observes
writes and releases its pins.

A separate live-Service fixture holds the existing inventory callback while
generating overflow, preserving file size/mtime during edits on both sides,
deleting one portable file under the default restore policy and creating
one-sided data plus an unseen nested directory. Inventory facts stay identical
and the comparison interval is one hour. Both trees recover their union, the
deleted file returns, and exactly one complete unresolved conflict preserves
the portable/local variants while retaining both original edits. After another
quiet owner-inventory interval, a new nested edit promptly reaches its peer.
There is no GUI or explicit reconciliation request after enrollment. Accepted
report: `/tmp/portable-drive-vm-watch-overflow-service-quiet/report.json`, with
three passing one-test groups and successful, non-timeout completion. An earlier
non-quiet fixture run is retained separately; the quiet run is authoritative.

All 138 ordinary backend tests pass (66 explicit fixtures ignored), together
with drive all-target clippy under denied warnings, targeted Rust formatting and
whitespace checks. Logs use `/tmp/portable-drive-watch-overflow-` with
`backend-final.log`, `final-build.log`, `host.log`, `service-quiet.log` and
`clippy-final.log`. The original-baseline architecture result remains REGRESSION:
4300 clone groups, 144 cycles, average CCN 3.48, maximum 274 and duplication
7.07%, with no additional reported hotspot, clone group or dependency cycle.
Report: `.fitness/portable-drives-watch-overflow-architecture.json`.
Registration-limit/retry, event loss during an active mutation and mount-inventory
loss remain unfinished; this checkpoint does not complete those broader rows.

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

## Image state across real reboots

New image effects persist an origin before entering native authority: a non-nil
boot UUID, known namespace and optional application-scoped SHA-256 fingerprint
of the configured machine ID. The origin file read runs on the retained worker,
outside SQLite's writer. Raw machine IDs are not persisted. Attachment records
retain the same host context. Optional fields decode older records conservatively;
the registry stays at version 3 and catalog/backup formats do not change.

A different kernel boot on the same identified host proves that the old kernel
mapping cannot still exist. Shared image evidence validates backup identity,
attachment origin and any recorded view before retiring only runtime metadata.
The transaction rechecks exact header/view/attachment values and any control
owner, clears the backup's mount/loop fields and removes its view, attachment,
journal and associated recovery notice together. Standalone attachments defer to
an existing journal. No old image/view/device path is opened, no native authority
is replayed, no reused loop is detached and no payload is removed. A legitimate
partial unmount may have a cleared header mount alongside its prior matching
view; live provider validation remains stricter. Existing policy refresh releases
the temporary whole-root optional-indexing barrier after recovery completes.

Current-boot attachments are excluded from the maintenance SQL selection; 2,202
such rows do not postpone an old record behind status pagination. Explicit
controls refuse to replace a known previous-boot origin before recovery, including
unidentified-host records which must remain protected. The worker repeats its
admitted-input check after origin binding. INSERT/UPDATE trigger cases prove
that both an admission-time replacement and a replacement during binding cause
zero native calls. Other ordinary tests cover scoped host derivation, unknown or
inconsistent origins, partial views, transactional rollback and changed evidence.
The backend passes 268 tests with 116 opt-in cases ignored in
`/tmp/portable-drive-image-boot-backend-final.log`. Affected backend/Files/CLI/daemon
all-target Clippy and runtime builds pass in
`/tmp/portable-drive-image-boot-clippy-final.log` and
`/tmp/portable-drive-image-boot-build-final.log`.

The new fixture retains its registry and images on an explicitly owned, pinned,
bounded regular virtual disk across two actual QEMU kernel boots. Only guest
filesystems are mounted. It creates three real states per source format: a
LoopSetup whose reply is deliberately not recorded, a ready read-only image and
a partial unmount blocked by another raw-device reader. After reboot it reuses
all recorded loop numbers for unrelated fixture files, starts the daemon without
native replay authority, waits for automatic metadata/policy recovery, checks
all image hashes and verifies every unrelated mapping and its bytes survive.
Both phases pass on FAT32, exFAT and NTFS in
`/tmp/portable-drive-image-boot-reboot-v2/{prepare,recover}/report.json`.

Disabling only the previous-boot retirement proof fails at the intended recovery
check on FAT32 and NTFS in
`/tmp/portable-drive-image-boot-negative-v1/recover/report.json`. Its exFAT attempt
encountered an earlier failed case's still-attached test device and is not counted
as evidence. An isolated exFAT rerun fails at the intended check in
`/tmp/portable-drive-image-boot-negative-v1/recover-exfat/report.json`.
These are expected failing controls, not successful acceptance runs. Restoring
the fix recovers all three formats from that same persisted negative-control
state in a later real boot:
`/tmp/portable-drive-image-boot-negative-v1/recover-repaired/report.json`.
All reports complete without a timeout. The final source additionally passes
12 native image/control/provider test invocations with ordinary destinations and
nine with Btrfs destinations in
`/tmp/portable-drive-image-boot-final-vm-native/report.json` and
`/tmp/portable-drive-image-boot-final-vm-btrfs/report.json`.

The Python disk-pin tests pass for pathname replacement, symlinks, FIFOs,
directories, short files and hardlink aliases. The pin is closed after the actual
QEMU process exits, including command/log setup failures. Python compilation,
shell syntax, changed-source rustfmt and whitespace checks pass. Test support is
committed separately as `c8c1f511`.

The local architecture comparison adds 823 NLOC, 32 net functions and 144
aggregate CCN relative to `pre-image-setup-recovery`; average/max CCN remain
3.50/274. Clone groups, cycles and duplication remain 4,321, 144 and 6.93%.
Consolidating shared evidence checks and guest helpers removed the initially
reported new hotspots and five net clones. No new production hotspot remains.
The tool still labels the aggregate growth REGRESSION and lists unchanged clone
regions in touched files; this growth implements the recovery proof and real-boot
failure fixtures. Report: `.fitness/portable-drives-image-boot-local.json`.
The original whole-branch architecture and final workspace gates remain open.

This is progress, not plan completion. Unknown current-boot outcomes cannot be
attributed from a same-user loop/image match alone: UDisks LoopSetup has no
request nonce in its returned object. Legacy records, missing host context and
inconsistent origins therefore remain protected rather than adopting or deleting
unidentified mappings. The remaining provider, identity, interruption, UI and
resource requirements above keep their original scope.

## Native reconnect and cloned-partition checkpoint

`service::tests::native_reconnect::native_reconnect_preserves_offline_edits_and_suspends_cloned_partition_ids`
uses a clean copied GPT image with a fresh partition UUID inside the disposable
native-filesystem guest. The runner's source is unmounted before copying and
restored immediately afterward. The same headless service stays running through
safe eject, detachment, relocation, offline edits, remount and clone ambiguity.
All loop devices, images and namespace mounts belong to this fixture; no host
block device or privileged host mount is used.

The fixture ejects through the daemon so its retained write/read owners retire
before normal unmount. It reserves the old loop number with an unrelated blank
fixture, then reattaches the same partition at a different device path. Another
private mount namespace performs the portable-side offline edit while the live
owner's namespace still observes an unmounted partition. Both report edits keep
their previous modification times and their ten-byte lengths. Local and portable
one-sided additions and a local deletion are also made while disconnected.

Remounting at a different directory converges automatically, without an explicit
Sync request: both complete conflict variants and one-sided additions exist on
both sides, the deleted file is restored under the default policy, and each
original retains its own edited contents. Persistent enrollment ID/evidence and
one conflict survive subsequent comparisons. Assertions verify changed device,
kernel disk sequence and mount path with unchanged stable identity/evidence.
Parent-drive capacity matches the complete GPT image and the selected partition
retains its distinct reported capacity, filesystem UUID and layout facts.

A second clean snapshot is attached with the same partition UUID. Discovery
reports two ambiguous entries. Before any explicit retry, automatic reconciliation
records the ambiguity; it does not install the queued local file. Explicit Sync
also refuses. The cloned raw image remains byte-for-byte unchanged. Detaching
only the clone automatically resumes synchronization to the remaining partition.

The final source passes all three formats in
`/tmp/portable-drive-native-reconnect-final/report.json`, with no timeout and a
successful QEMU exit. FAT32, exFAT and NTFS3 take 17.44, 16.09 and 16.03 seconds
respectively. `NATIVE_RECONNECTED_DEVICE` markers show the actual old/new device
and mount paths. The earlier corrected fixture also passes all three formats in
`/tmp/portable-drive-native-reconnect-v2/report.json`. The first fixture attempt
used ordinary unmount despite retained owner handles and copied mounted NTFS;
its busy/dirty-image failures in `...-v1/report.json` are not accepted evidence.

The ordinary backend passes 268 tests with 117 opt-in cases ignored in
`/tmp/portable-drive-native-reconnect-backend-final.log`. Backend all-target Clippy
passes in `/tmp/portable-drive-native-reconnect-clippy-final.log`. The shared test
enrollment helper now selects the addressed drive/subtree/local pair rather than
popping an arbitrary status row; native, ordinary and unavailable-root fixtures
reuse it. Changed-source rustfmt and whitespace checks pass.

Relative to `pre-native-drive-reconnect`, the architecture report adds 335 NLOC,
13 functions and 25 aggregate CCN, entirely in test support. Average/max CCN,
duplication, clone groups and cycles remain 3.50/274, 6.93%, 4,321 and 144. There
is no new production hotspot; consolidating enrollment removed the initial new
clone. The tool still classifies the aggregate test growth as REGRESSION and
lists unchanged touched-file clone/cycle regions. Report:
`.fitness/portable-drives-native-reconnect-local.json`.

This closes the specific changed-device-path requirement in 1.1 and strengthens
clone and reconnect evidence in 1.2/2.2/2.7/7.2. It does not prove physical removal
during accepted writes, fallback enrollment disclosure, failed inventory probes,
or every remaining interruption requirement. Those items remain open.

## Filesystem fallback checkpoint

`Drive::backup_bucket` now owns the existing stable naming rule shared by the
backend and file manager: an available PARTUUID takes precedence, otherwise the
name is `fs-{filesystem UUID}`. It validates a single ASCII directory component
without accessing a filesystem. Empty, absolute, traversal, separator, NUL and
non-ASCII identifiers are rejected; an invalid selected PARTUUID cannot redirect
creation outside the destination. Neither UUID returns no stable key, retaining
the existing per-explicit-backup unidentified-directory behavior. On-disk bucket
names, protocol representation, registry and manifest versions remain unchanged.
The shared naming change is committed as `f48f1130`.

The file manager previously browsed the overall backup root for filesystem-UUID
media even though the backend wrote to its `fs-UUID` child. Browse now uses the
shared rule. Pairing and backup review disclose a missing partition UUID, the
filesystem UUID and the persistent drive identity; review shows the exact backup
folder. Explicit Continue/Start remains the enrollment/backup action, with no
new confirmation step or startup backup-directory prompt. Pairing is disabled
for ambiguous identities or when both UUIDs are missing; the backend independently
refuses the latter before creating a target directory or pair record.

The native fixture uses actual whole-volume FAT32, exFAT and NTFS3 loops with no
partition table. Its owned inventory seam supplies their observed UUID/mount
facts to the normal headless service. It verifies that discovery alone starts
neither enrollment nor backup; explicit enrollment performs union, and explicit
backup-directory selection is remembered. After actual service shutdown/restart,
the same pair and evidence remain, both backup versions share the same `fs-UUID`
parent, old bytes remain intact and checksummed manifests keep PARTUUID absent.
Source directory entries contain only the intended user files, with no local
baseline, manifest or repository metadata added to the media. All three cases
pass without a timeout in `/tmp/portable-drive-fallback-native-v1/report.json`.
This is filesystem/identity persistence evidence, not a new physical-hotplug claim;
the preceding native GPT reconnect/clone checkpoint supplies that distinct gate.

The core drive subset passes 15 tests (one unrelated opt-in case ignored) in
`/tmp/portable-drive-fallback-core-v1.log`. Files passes 15 drive tests in
`/tmp/portable-drive-fallback-files-tests-v1.log`. The backend passes 269 tests,
with 118 opt-in cases ignored, in
`/tmp/portable-drive-fallback-backend-final.log`. Core/backend/Files/CLI/daemon
all-target Clippy, including `clip-drives/test-support`, passes in
`/tmp/portable-drive-fallback-clippy-v1.log`. Runtime builds pass in
`/tmp/portable-drive-fallback-runtime-build-v1.log`. The final fixture startup
refactor separately builds and passes Clippy in
`/tmp/portable-drive-fallback-fixture-{build,clippy}-v2.log`.
Core documentation builds with warnings denied in
`/tmp/portable-drive-fallback-doc-v1.log`; the rendered API contract is checked.

The test-support fixture's optional `--filesystem-identity` mode supplies an
absent PARTUUID without changing production discovery. Both private dark/light
Files scenarios pass with 31 captured observations each in
`/tmp/portable-drive-fallback-ui-{dark,light}-v4/report.json`. They explicitly
pair through the GUI, preserve the initial union, cancel a first lazy backup
review without setting its preference, then create the file backup. The review
shows the filesystem identity and `fs-fixture` folder. Image review also remains
readable and cancelable. Browse enters the actual saved bucket. Source removal
and return preserve the pairing, and a real fixture-service restart preserves
its pair ID, selected backup directory and subsequent version parent.

A filename alone could also be visible in a child folder preview. The final
address-field oracle therefore reads only the rendered address bar. It is applied
to both actual Browse and post-restart captures in both appearances, asserting
exactly `.../fixture/backups/fs-fixture`; results are in
`/tmp/portable-drive-fallback-address-proof.json`. These exact address assertions
are now part of the scenario. Pair, review and Browse captures are also visually
inspected in both appearances, with all actions visible. The initial UI attempts
failed for missing OCR environment configuration, a small-button lookup and a
wrapped-label OCR expectation respectively; `...-v1`, `...-v2` and `...-v3` are
not counted as passing scenarios. The accepted runs use the existing local
`/tmp/little-clip-tessdata` through `TESSDATA_PREFIX` and verify the rendered button
and menu text rather than guessing an unobserved control location.

The local architecture comparison against `pre-drive-fallback-identity` adds
350 NLOC, ten net functions and 40 aggregate CCN. Average/max CCN remain 3.50/274;
clone groups and cycles remain 4,321 and 144. Duplication changes from 6.93% to
6.92%. There is no new production hotspot. Separating fixture startup arguments
from its IPC loop reduces that existing example main from CCN 24 to 23 instead
of increasing it to 27. The tool still labels the aggregate feature/test growth
REGRESSION and lists unchanged clone/cycle regions in touched files. This growth
covers the shared identity rule, disclosure, native persistence and UI navigation
oracles. Report: `.fitness/portable-drives-fallback-identity-local.json`.

This closes the explicit fallback-disclosure/enrollment requirement in 1.2.
Other incomplete identity, interruption, resource, provider and UI rows retain
their original requirements. Final workspace and whole-branch architecture gates
remain open.

## Live configuration and retained RAM checkpoint

The daemon's `ReloadConfig` previously discarded the drive admission response and
reported all configuration applied. It now names the accepted asynchronous
operation and directs the caller to `clipctl drives status`. Admission failure
reports that the other application settings applied while drive settings did
not. An unavailable optional drive service is disclosed without preventing the
other application settings from being reloaded. The wire variants remain the
existing Success/Error responses; the IPC handler never waits for destination
validation to finish.

The new daemon test uses its actual Unix-stream IPC handler and private isolated
configuration. A malformed RAM limit is accepted as a job, then fails without
changing drive settings; a subsequent valid limit completes. Both receipts name
their retained operation. Closing drive admission produces an explicit partial
reload error while the independent history setting still applies. The optional
unavailable-service receipt is also checked. All 37 daemon main tests pass:
`/tmp/portable-drive-config-reload-daemon-all-v2.log`.

The public configuration matrix submits fifteen invalid field values and checks
unchanged runtime/durable settings before any new backup destination is created.
The backend suite passes 270 ordinary tests, with 119 environment-dependent tests
explicitly ignored: `/tmp/portable-drive-config-quota-backend-v2.log`.

The bounded-RAM native fixture runs one explicit test for each FAT32, exFAT and
NTFS source. Two dirty paused RAM copies remain readable/writable while five
otherwise valid quota reductions are refused: per-session bytes, aggregate
bytes, per-session inodes, aggregate inodes and session count. Saved settings,
paths, reservations, pause/deletion decisions and an open file's identity remain
intact. Exact retained limits are accepted. Changing the RAM directory affects a
new third copy, while old views keep their locations. A raised available-memory
minimum and lowered drive-capacity threshold refuse new admissions but permit
existing copies to flush. The newly accepted catalog limit produces an explicitly
partial one-entry catalog. Three actual file backups use the new destination;
explicit retention leaves two versions and protects current. Restart loads the
saved TOML configuration and reopens all three copies with their data and policy
intact, before explicit cleanup.

Accepted native evidence is
`/tmp/portable-drive-config-quota-native-v2/report.json`: success=true,
timed_out=false, QEMU exit 0, three executed/passed tests, fifteen named quota
refusals, and all three filesystem results 0. The first VM run selected an older
Cargo artifact and executed zero matching tests; it is rejected as acceptance
evidence. The corrected invocation verifies the exact test appears in the chosen
binary's listing before boot, and the resulting log verifies three executions.
No host mount, block device or attached removable disk is used. The typed TOML
fixture adds only a development dependency on the existing workspace TOML version.

Affected all-target Clippy with the fixture feature and warnings denied passes:
`/tmp/portable-drive-config-reload-clippy-v1.log`. The daemon builds successfully:
`/tmp/portable-drive-config-reload-build-v1.log`. Changed Rust files pass rustfmt
and the diff passes whitespace checks.

Architecture comparison against `pre-drive-config-reload-evidence` adds 526 NLOC,
13 functions and 32 aggregate CCN. Average/max CCN stay 3.50/274, duplication stays
6.92%, and cycles stay 144. There is one new clone group: four lines of Unix-stream
test connection setup shared with the existing history-reload test, reported
with surrounding context as seven lines. The new test already reuses the existing
IPC serializer/reader and isolated configuration harness; no second protocol or
production mechanism was added. A separate wrapper solely for these four setup
lines would expand the test interface without simplifying behavior. Other
reported clones are unchanged regions in touched files; the reordered cycle
members do not establish new cycles. No new hotspot is introduced. Local report:
`.fitness/portable-drives-config-reload-local.json` (the tool classification is
REGRESSION for that test setup clone). This closes configuration item 1.3; the
remaining resource-pressure, interrupted-operation, provider, UI and final
whole-branch gates keep their existing scope.

## Confirmed synchronization receipts checkpoint

Operation `files`/`bytes` previously mixed scan and copying progress. Explicit
reconciliation now retains a separate optional `sync_completion` summary and
per-path `sync_receipts` rows. Confirmed installs/removals, their destination side,
raw relative path, resulting fingerprint and installed bytes commit with the
common baseline in one existing SQLite FULL-synchronous transaction. Multiple
directory removals share that transaction. No file I/O happens while it is held.
Inventory progress cannot overwrite the summary, and a later initialized
catch-up pass adds to the same summary rather than resetting it. This adds no new
mutation mechanism, worker pool or dependency.

Receipts are confirmed lower bounds. A file installed before a failed baseline
or receipt transaction is retained but remains unconfirmed. Restart retains the
old operation and fresh reconciliation compares both sides before retrying;
an equal unconfirmed file need not be replaced or attributed to the new job.
Missing or finished operation owners refuse entry before the reconciliation
starts mutating. Conflict variants without a common baseline, RAM exports and
backup-item journals remain separate unfinished work. No receipt is promoted
into provider-revision evidence or source payload metadata.

The existing baseline interleaving fixture now tests six failures in each copy
direction with both absent and replaced destinations: cancellation, source
loss, source edit, destination edit, baseline transaction failure and receipt
transaction failure. The last two occur after file B is actually installed.
Only file A's baseline/receipt/summary are durable, B's old baseline is preserved,
and reopening/reconciling preserves already installed descriptors. Separate
checks cover file/directory deletion receipts and lossless non-UTF-8 path bytes.
Four owned child workers are killed after the first confirmed installation;
actual service startup marks their operations interrupted with the same summary
and receipts, and retry preserves the installed file. These are storage-worker
crash checks, not claims of physical-device power-loss testing.

A public Pair/Sync fixture injects a receipt failure after B is installed, checks
the failed paused enrollment and confirmed A, restarts the service, then verifies
that a new job confirms only C. The original failed job keeps its total, B is
recognized as equal and the held A descriptor keeps its FileSource revision.
RAM population tests now create an admitted-operation fixture and verify totals
across one-pass negative control, initialized catch-up, failure and resume.
The explicit metadata-size report accounts for receipt payloads separately from
optional catalogs, including raw path lengths, null deletion fields and decimal
counter lengths, without mutating the source or registry.

Native evidence is `/tmp/portable-drive-sync-receipts-native-v2/report.json`:
success=true, timed_out=false, QEMU exit 0 and all FAT32/exFAT/NTFS results 0.
Each filesystem runs five baseline/receipt tests, one public-job test and two
real RAM-population tests: 24 top-level passes, 72 fault cases and 12 killed
workers in total. The child-only probe is ignored in the ordinary test listing
and invoked explicitly by its owning parent. Test names were verified in the
selected Cargo binary before boot, and nonzero execution counts were checked
afterward. No host privileged mount, raw device or attached media is used.

Backend final ordinary checks pass 275 tests, with 120 environment-dependent
tests explicitly ignored:
`/tmp/portable-drive-sync-receipts-backend-final-v3.log`. Daemon main tests
pass all 37: `/tmp/portable-drive-sync-receipts-daemon-check-final.log`.
Core drive tests pass 16 (one environment-dependent ignore) and Files drive
tests pass 15: `/tmp/portable-drive-sync-receipts-host-v1.log`. This includes the
legacy-operation serialization check: old progress stays separate from unknown
completion and new summaries round-trip. Runtime and fixture builds pass in
`/tmp/portable-drive-sync-receipts-{runtime,fixture}-build-final.log`.
Additive receipt-schema migration is tested against registry versions 1–3,
preserving their raw baselines and legacy operation JSON without inventing
confirmed totals. Strict core rustdoc generation passes and renders the new
`SyncCompletion` type: `/tmp/portable-drive-sync-receipts-core-doc-final.log`.
Final daemon and fixture builds pass in
`/tmp/portable-drive-sync-receipts-{daemon,fixture}-runtime-final.log`.
Named-event options retain the original forced-fresh directory fallback; the
poisoned-cache tests now deliberately request reuse and still prove fresh scans.
Changed Rust files pass rustfmt and the diff passes whitespace checks. The
acceptance script passes Python compilation.
Affected all-target Clippy, including the fixture feature and warnings denied,
passes in `/tmp/portable-drive-sync-receipts-clippy-final-v3.log`. Its first run rejected
one complex test tuple type; the test now directly verifies SQL null deletion
fields instead of nesting two unnecessary Options.

Private Files acceptance passes 33 observations in each appearance:
`/tmp/portable-drive-sync-receipts-ui-dark-v1/report.json` and
`/tmp/portable-drive-sync-receipts-ui-light-v2/report.json`. Actual Jobs captures
show **Confirmed common writes: 4 installed · 0 removed · 112 bytes**, with its
exclusion explanation wrapped inside the dialog and scan progress separately
labelled. IPC checks match those counts. The scenarios continue through lazy
backup cancellation/publication, exact filesystem-UUID bucket browsing, drive
removal/return, service restart and a second backup. Both retained current
references include the reconnected edit; supplementary proof is
`/tmp/portable-drive-sync-receipts-ui-proof.json`.
The first light run passed the receipt view but failed its final visibility
oracle because an inline SQLite preview pushed `current-files.json` below the
viewport. The corrected oracle checks the exact displayed address and current
reference's new version/path on disk; it does not require an off-screen row.
Both theme screenshots were visually inspected. The backend is committed as
`e8a0d5fe`; the display/acceptance changes are a separate reviewable commit.

The local architecture comparison against `pre-drive-sync-receipts` adds
623 NLOC, thirteen functions and 70 aggregate CCN. Average/max CCN remain
3.50/274; duplication changes 6.92%→6.91%, clone groups remain 4,322 and cycles 144. Existing
`changed` and `synchronize_with` each gain one fallible operation-owner check
(CCN 22→23 and 31→32). That required ownership validation is retained; directory
completion instead batches effects with its baseline rather than adding
separate per-root transaction branches. No new production hotspot, clone group
or dependency cycle appears. Reported clones are unchanged regions in touched
files and reordered cycle members are not new dependencies. The tool's aggregate
classification remains REGRESSION for those two required checks. Report:
`.fitness/portable-drives-sync-receipts-local.json`.

Items 1.4, 1.5 and 7.1 remain Partial. Unconfirmed intents, conflict/export/backup
item completion, persistent operation coverage and the other original gates
remain open; confirmed ordinary-sync effects do not close those broader scopes.

## Native full/read-only RAM writeback checkpoint

The existing full-media/preflush scenario now reuses an owned source fixture for
either its bounded ext4/FUSE reference or a native guest FAT32/exFAT/NTFS tree.
Native admission uses the actual 256 MiB source/parent capacity from the guest.
The ordinary reference still reserves space outside its selected subtree. Native
filler lives inside the private source tree, sorts after the edited file, and is
removed before successful retry. The failed copy must stop before copying that
filler into RAM. The test verifies at most 1 MiB portable free space and more than
the 8 MiB edit's size available in the independent RAM filesystem, avoiding a
false pass from exhausting RAM instead of the portable filesystem.

A second mode uses the existing boot- and mount-identity-checked GuestMount
read-only/remount guard. It changes the actual backing filesystem to read-only
and verifies the kernel flag while deliberately leaving discovery's read-only
field false. The observed EROFS therefore proves filesystem enforcement rather
than only a cached inventory refusal. Visibility of the two existing fixture
methods is widened within the test-only module; no second mount helper or
production mechanism is added.

Both modes exercise the same complete scenario: a paused dirty RAM copy survives
failed explicit flush and failed eject preflush with its original portable file,
full edit, pending work and distinct previous confirmation intact. Staging is
retired and the filler never appears in RAM. No unmount callback is reached while
preflush fails. Freeing space/restoring writability permits verified flush and
clears the pending paths. A later paused edit is actually installed before a held
eject callback; queued discard stays owned, another partition's control remains
responsive, eject finishes and unconfirmed discard still refuses. The RAM view
remains editable after the fixture reports the portable side offline. These
checks cover the preflush/ownership barrier; their final unmount callback is the
existing fixture authority, not a claim of new UDisks or physical-device testing.

Final native evidence:
`/tmp/portable-drive-native-writeback-v2/report.json`. It reports success=true,
timed_out=false, QEMU exit 0 and each native filesystem result 0. All six explicitly
selected tests executed and passed. Logs contain six ENOSPC and six EROFS job
refusals; all three full-media runs report 1,048,576 portable bytes free. All six
RAM runs report enough independent space for the 8,388,608-byte edit. The first
native run also passed; the final run includes the explicit headroom/filler
oracles and the common reference/native diagnostic label. Owned images and
privileged mounts exist only inside the disposable guest. No host mount, attached
card, raw device or system service is changed.

The ordinary FUSE reference also executes and passes:
`/tmp/portable-drive-native-writeback-fuse-reference-v1.log`. Backend ordinary
checks pass 275 tests, with 121 environment-dependent tests explicitly ignored:
`/tmp/portable-drive-native-writeback-backend-v1.log`. All-target backend Clippy
with the fixture feature and warnings denied passes:
`/tmp/portable-drive-native-writeback-clippy-v1.log`. Changed Rust files pass
rustfmt and the diff passes whitespace checks.

Local architecture comparison against `pre-drive-native-writeback-failures`
adds 122 NLOC, seven functions and thirteen aggregate CCN, all in tests. Average
and maximum CCN stay 3.50/274, duplication stays 6.91%, clone groups stay 4,322
and cycles stay 144. The targeted three-file measurement has no hotspot or clone
group. The tool's REGRESSION output points at an unchanged clone in a file with
two visibility edits and reordered members of unchanged dependency cycles;
there is no new duplication or production complexity. Sharing the existing
scenario avoids duplicating its flush/eject/discard assertions for the new
failure mode. Report: `.fitness/portable-drives-native-writeback-local.json`.

Items 3.5, 3.6 and 7.3 retain their remaining interrupted-I/O/transport, cancellation
and mount-option requirements. Full/read-only native proof does not substitute
for removal during an active native write or final whole-branch acceptance.

## Owned USB transport fixture checkpoint

The VM runner now accepts a separate `--removable-disk` owned regular artifact,
exposed through an emulated USB mass-storage controller. It reuses the existing
64 MiB–8 GiB, owner, regular-file, no-symlink and single-link admission and keeps
the descriptor inherited until actual VM exit. State/removable aliases are
refused by retained inode identity. A short mode-private socket carries QMP;
no host USB device, block node, network or elevated operation is admitted.
Explicit guest markers can remove/reinsert only the fixture frontend. The block
backend remains owned and each replug creates a fresh frontend identity.

The controller negotiates capabilities, matches reply IDs, bounds messages and
event retention, and requires the selected frontend's deletion event separately
from command acknowledgement. Consumed or older-generation events cannot confirm
a later removal. Reports retain every control attempt, its frontend, completion
and controller error. Failed or incomplete controls fail the overall report even
if the process/guest markers otherwise appear successful. Ordinary execution
still retains nonzero exit and timeout outcomes; isolating process execution
keeps disk admission and report assembly separate from owned process lifetime.
Protocol/setup references are the
[QMP specification](https://www.qemu.org/docs/master/interop/qmp-spec.html) and
[USB device guide](https://www.qemu.org/docs/master/system/devices/usb.html).
Installed QEMU 11.1.3 device help and actual runtime replies supply the local
property/event evidence.

`vm_usb_probe.sh` formats only the explicit owned artifact as FAT32 and checks
two clean unmount/unplug/replug cycles. Kernel block nodes actually disappear;
the reused `/dev/sda` spelling has disk sequences 1, 2 and 3. UUID, capacity and
payload survive both returns. Four control receipts bind the initial frontend
and two fresh generations. Final proof:
`/tmp/portable-drive-usb-hotplug-generation-final/report.json` and
`/tmp/portable-drive-usb-hotplug-proof.json`. Success=true, timed_out=false,
QEMU exit 0 and all four controls complete.

Negative controls are rejected:
`/tmp/portable-drive-usb-hotplug-duplicate-generation-final/report.json` records
the completed first unplug and refused duplicate, reports its error and reaps the
VM. `/tmp/portable-drive-usb-hotplug-noop-generation-final/report.json` uses an
uncommitted mock that suppresses the mutation; the guest fails its actual-node
disappearance check despite QEMU exiting 0, and the pending control remains
incomplete. Disabling the action therefore cannot produce accepted evidence.
The earlier oversized serial prototype failed before boot and was corrected to
the locally supported serial length; it is rejected as evidence.

Thirteen Python checks pass: eight protocol/controller cases and five
owned-disk/execution cases. They cover event-before-reply ordering, wrong device
and generation, stale consumption, missing completion, foreign replies, explicit
errors, repeated state, controller failure, actual process retirement, timeout,
pinning across pathname replacement and state/removable alias refusal. Logs:
`/tmp/portable-drive-usb-test_drive_hotplug.py-final-v2.log` and
`/tmp/portable-drive-usb-test_portable_drive_vm.py-final-v2.log`. The unchanged
non-USB formatter/remount route also passes all three filesystems:
`/tmp/portable-drive-usb-hotplug-legacy-final/report.json`. An earlier compatibility
command chose `/bin/true`, which is not an initrd applet; its three command
failures were correctly rejected. The corrected `/usr/bin/true` route passes.
Python compilation, shell syntax and whitespace checks pass.

Local architecture comparison against `pre-drive-usb-hotplug-fixture` adds
350 NLOC, thirty functions and 93 aggregate CCN in development tooling/tests.
Average/max CCN remain 3.50/274, duplication 6.91%, clones 4,322 and cycles 144.
There is no newly reported local hotspot or clone. Earlier report-assembly
growth was removed by giving process execution its own tested responsibility;
the final targeted maximum CCN is below the hotspot limit. The tool's aggregate
REGRESSION includes reordered members of unchanged cycles and a recalculated
score for an unchanged vendor chokepoint, outside the modified implementation.
Report: `.fitness/portable-drives-usb-hotplug-local.json`.

This establishes controlled real guest device disappearance/reappearance. The
probe unmounts cleanly before removal; it does not close the active native-write
or application recovery requirements in 3.5/3.6/7.3. Those are the next users of
the fixture, alongside the original unfinished coverage/provider/UI gates.


## Interrupted USB staging recovery checkpoint

The previous status-only turn was no progress. This turn implements and verifies
recovery for a production bug exposed by actual emulated USB removal: a failed
`NamedTempFile` unlink on the absent card left a partial file that the next union
copied into RAM. The failing FAT32 run is
`/tmp/portable-drive-usb-writeback-v2/report.json`; it reached the intended leaked-
staging assertion after native EIO, retained RAM, export and restart. Its later
cleanup panic is not accepted as passing evidence.

Shared checked copies now allow progress receipts to fail before the next payload
write (commit `25613ad6`). A regression proves the first 256 KiB write remains
unpublished and subsequent chunks are not written after a receipt failure.
The Files caller still records ordinary byte progress through that fallible API.

Ordinary sync and conflict-file copies now use a caller-owned local staging
journal. The root primitive owns admitted descriptors, streaming/reflink output,
metadata and installation; it has no Store/sync dependency. The journal commits
a UUID name before exclusive creation, then a separate creation/empty-prefix
receipt. Every completed chunk records its cumulative SHA-256 prefix before the
next write. Successful installation and parent flush retire the record/proofs.
The first intent atomically creates its proof table and upgrades the operational
registry to version 4, which pre-journal daemons do not accept. Backup manifests
and catalogs retain their own existing versions.

Before either a named or complete user reconciliation, pending receipts require
a bounded metadata-only inventory of the affected enrolled root. Known names can
be found after parent renames and ASCII case normalization. Cleanup verifies the
persisted length and checksum, even when the original RAM file has since changed.
Both its original and deterministic quarantine name are known before deletion,
so another interruption during cleanup does not invent a new untracked name.
Uncertain creation, unproven lengths, changed contents, symlinks and duplicate
candidates retain data and stop union; proof failures identify the candidate's
path for review. These are safe refusals, not automatic repair claims.

Host staging tests exercise empty/partial/full prefixes in both directions,
renamed parents, normalized names, subsequent source edits, changed candidates,
creation uncertainty, symlinks, duplicate names, unknown persisted lengths,
quarantine/absent-stage recovery, reopened version-4 state and receipt faults.
The complete backend suite passes 279 enabled tests with 122 opt-in cases ignored
in `/tmp/portable-drive-usb-stage-backend-v5.log`. Shared stream tests pass three
cases in `/tmp/portable-drive-usb-stage-core-v3.log`; the Files transfer suite
passes four existing preservation/cancellation cases in
`/tmp/portable-drive-usb-stage-files-copy-v5.log`. Affected all-target Clippy with
the fixture feature and warnings denied passes in
`/tmp/portable-drive-usb-stage-clippy-v6.log`.

The final native application run passes on vfat, exfat and ntfs3 in
`/tmp/portable-drive-usb-stage-v6-vfat/report.json`,
`/tmp/portable-drive-usb-stage-v6-exfat/report.json` and
`/tmp/portable-drive-usb-stage-v6-ntfs3/report.json`. Each report records exit 0,
no timeout and exactly one completed unplug/replug sequence. The test fsyncs a
1 MiB prefix of an 8 MiB write before requesting actual QMP frontend deletion;
the native write fails with EIO, preserves the original target and old sync
confirmation, and leaves the RAM copy browsable/dirty. Another partition's rule
request succeeds while the writer is held. After further same-file offline edits,
full export and service restart, replug exposes the actual old partial file.
Recovery removes it before union, installs the newer RAM bytes and offline-added
file, and leaves no staging filename in either tree. The verifier and exact test
binary digest are in `/tmp/portable-drive-usb-stage-v6-proof.json`.

The existing native full-media and kernel-read-only writeback/eject fixtures also
pass again with the new journal on all three source formats (six test
invocations) in
`/tmp/portable-drive-usb-stage-v6-pressure-regression/report.json`. The guest exits
0 without timeout; every filesystem reports two passed tests and the expected
ENOSPC/EROFS refusals before successful retry.

The first exFAT attempt selected the tool tree's userspace mount helper and failed
its read-only-to-writable remount; that failed run is not kernel-driver evidence.
The guest now uses `mount --internal-only`, and the Rust fixture checks the actual
mounted filesystem before writing. All final runs use the specified kernel
filesystem. No host block node, privileged mount or system service is used.

Architecture review first found new recovery hotspots (CCN 26 and 18) and two
repeated fixture blocks. Recovery now owns its two snapshot lifetimes across a
bounded receipt walk, separates ownership/content attestation from deletion, and
shares write/confirmation fixture setup. No newly introduced function exceeds the hotspot threshold after those
revisions; clone-group and cycle counts are unchanged. The existing shared
stream gains one fallible-progress branch (CCN 16 to 17), which Valknut also
flags as a newly classified high-complexity entity. Aggregate growth at the final reviewed revision
is 1,073 NLOC, 41 functions and 160 aggregate CCN; average/max CCN remain 3.50/274,
duplication changes 6.91% to 6.90%, and clones/cycles remain 4,322/145. Required
fallible-receipt handling and pre-scan recovery add branches to existing shared
stream/named/full-sync functions. The tool still reports REGRESSION, including
reordered unrelated cycles; the feature-wide architecture gate remains open.
Its apparent `install` improvement conflates two same-named functions and is not
claimed as a measured simplification. Report:
`.fitness/portable-drives-usb-stage-final.json`.

Items 1.4, 3.5, 3.6 and 7.3 remain Partial. This covers ordinary/conflict regular-
file staging and controlled transport removal, not every persisted torn length,
electrical power failure, directory/link mutation, second-removal cleanup,
cancellation/eject/discard combination or conflict/export/backup item receipt.
The other provider, identity, UI and final acceptance gates remain unchanged.
