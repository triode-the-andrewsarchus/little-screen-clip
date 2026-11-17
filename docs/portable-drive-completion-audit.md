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
| 1.6 | Partial | Pinned roots and overlap policy project bind subtrees into observed filesystem coordinates. Descriptor-free scheduling scopes now retain that observation after pins are retired; they cannot authorize payload access. Watch/cache retirement covers overlapping observed filesystem regions, including bind aliases, while retaining unrelated watches. Pure overlap/projection tests and a real normal FUSE unmount with a retained scope pass; a restarted native fixture also proves startup-configured file backups refuse private RAM backing. Real bind/case-fold aliases, remaining managed-root coverage, image destinations and the full authorized-device lifetime checks still need the complete matrix. |
| 2.1 | Partial | Persistent subtree pairs and baselines exist; strengthen fault-injection evidence for committing both sides. |
| 2.2 | Partial | Native FAT32/exFAT/NTFS3 union and concurrent same-timestamp edits preserve both conflict variants; FAT32/exFAT case-fold collisions explicitly fail with both local originals retained. Remaining remount/event-loss and fault matrices still need evidence. |
| 2.3 | Partial | Watch overflow now clears the root observation epoch; coarse/unknown filesystems never reuse full-scan fingerprints. Direct filesystem and loss/reconnect matrices remain. |
| 2.4 | Partial | Persistent collision-safe complete conflict variants tested; provenance and repeat behavior need a full matrix. |
| 2.5 | Partial | Deletion and delete-versus-edit tests exist; event-loss and cancellation matrices remain. |
| 2.6 | Partial | Native file/directory name tables verify exact directory entries and bytes, or explicit refusal with originals retained. NTFS3 alternate-stream paths and FAT32/exFAT trailing dots/spaces are refused before installation. Native FAT32 oversized-file refusal passes; link/metadata omissions, alternate mount options and ntfs-3g remain. |
| 2.7 | Partial | Changed-file reconciliation, overflow invalidation and independent-drive scheduling tests pass. Coalesced path batches are shared and capped at 4096 paths/1 MiB; registration is incremental with 8192-watch/16 MiB/depth-256 budgets. Real overflow delivery matrices remain. |
| 3.1 | Partial | Capacity, MemAvailable and expanded contents are checked before admission; configurable per-session/aggregate byte, inode and session reservations now exist. Native quota and aggregate refusal tests pass; full pressure matrix remains. |
| 3.2 | Partial | Verified tmpfs path now falls back from the default /tmp to XDG_RUNTIME_DIR and /dev/shm. Explicit disk-backed override refusal is tested; fallback branch still needs an isolated non-tmpfs-/tmp acceptance environment. |
| 3.3 | Partial | Streaming initial population and incomplete state exist; a second reconciliation after installing watches catches earlier edits. Deterministic mid-population change/failure injection remains. |
| 3.4 | Complete | RAM sidebar shows connection state and bounded observed pending-path counts, with explicit full-check/unknown states. Menus and Jobs show the stable drive ID, RAM path and last confirmed drive sync separately from export history. Native removal/offline restart/reconnect and dark/light UI acceptance pass; acknowledgements preserve newer events and exports cannot advance drive confirmation. Counts remain advisory and never authorize eject/discard. See the RAM pending-status checkpoint. |
| 3.5 | Partial | Real bounded-media ENOSPC and controlled removal during a confirmed partial FUSE write pass. Failure/cancellation retain the original portable file, complete RAM edit, pending work and previous confirmation. Reconnect preserves both concurrent conflict variants and converges after catch-up. Physical USB/device-transport and native FAT/exFAT/NTFS interrupted-I/O matrices remain. |
| 3.6 | Partial | Eject includes paused ready RAM mirrors and refuses incomplete population/failed preflush. Native tests also queue eject and unconfirmed discard behind a held write, preserve ownership through cancellation, keep unrelated controls responsive and retain terminal results after reconnect. Broader read-only, dirty-session and eject/discard cancellation matrices remain. |
| 3.7 | Partial | Native quota, restart, directory-fsync kill and cancellation fixtures pass. Bounded guest tests prove available-memory admission refusal, underlying-tmpfs exhaustion/recovery, acknowledged external writes retained across abrupt daemon death while held open, 13 namespace/metadata edits without later data writes/fsync, and immutable-flag recovery with correctly refused truncation. The helper propagates eviction/operation errors and flushes successful mutations before acknowledgement. Interrupted unacknowledged operations and broader concurrent pressure matrices remain. |
| 4.1 | Partial | Lazy selection produces a five-minute review bound to retained partition/destination roots. Cancel/expiry, replaced roots, changed partition facts, one-shot destination after preference changes and failed preference writes are backend-tested. Pending reviews are bounded and released before unmount. Dark/light focused UI runs prove cancellation, file backup publication, and image review; alias and broader failure matrices remain. |
| 4.2 | Partial | Durable checksummed SQLite manifests and filesystem/registry current-version references survive reference-installation faults; pruning removes matching manifests and protects current across clock changes. Admission/execution check working headroom; independent images require their full size estimate before unmount, while Btrfs permits shared allocation with explicit uncertainty. Detailed file-allocation estimation remains. |
| 4.3 | Partial | Per-file checks now recollect changed files within three attempts, update the manifest to the installed revision and preserve current on repeated changes, cancellation or type replacement. Same-size/preserved-mtime races pass on real FAT32/exFAT/NTFS guest mounts; real Btrfs extent sharing and old bytes remain intact. Review and completion text disclose live collection rather than an atomic source snapshot. Metadata fidelity/omission records and optional quiesced read-only collection remain. |
| 4.4 | Partial | Real read-only Btrfs version snapshots and extent sharing passed; capability failure/admission matrix remains. |
| 4.5 | Partial | Real 4 KiB edits allocate 256 KiB, old bytes intact; strengthen final target verification evidence. |
| 4.6 | Partial | Explicit retention and mounted-provider refusal exist; open-provider/prune interruption matrix remains. |
| 5.1 | Partial | Preflush and unmount run as retained stages. Real owned FUSE integration proves external-reader busy refusal, successful pin retirement, independent RAM writeback during a held unmount and retained completed eject after cancellation. Image cancellation after unmount prevents copy admission. Admission retains the device-node pin and checks its disk sequence across authorized image reads. Reviewed requests show exact capacity/destination/free bytes and bind the selected roots; non-Btrfs full-image space failure precedes unmount. Real guest UDisks/PolicyKit authorization, busy-source refusal and successful preflush/unmount/image/restoration now pass. Interactive password-agent and hotplug/replacement races remain. |
| 5.2 | Partial | Isolated real UDisks/PolicyKit tests now prove authorized image reads, unprivileged denial and busy-device refusal on FAT32/exFAT/NTFS partitions. The desktop authentication-agent/password interaction remains a separate UI gate. |
| 5.3 | Complete | Exact authorized-device/image hashes, partition sizes, durable publication and checksummed source manifests pass on real FAT32/exFAT/NTFS GPT partitions. Manifests retain typed partition number/start/byte offset/logical sector size/type/label/flags and immediate containing-table type/identifier; unknown fields remain unknown. Native tests assert actual GPT layout and exact manifest retention. See the partition-layout checkpoint and its guest report. |
| 5.4 | Partial | Real Btrfs image delta and exact nonzero sectors pass; real non-Btrfs partition images now pass on FAT32/exFAT/NTFS with exact source SHA-256 and retained earlier images. Mount-option and interrupted-device matrices remain. |
| 5.5 | Partial | Optional mounted-state restoration now retains actual cleanup completion after imaging, failure or cancellation, refuses changed device lifetimes and reports separate restoration failures without losing completed backups. Owned authority and real FUSE fixtures pass; Real authorized image success and mounted-state restoration now pass on all three guest source formats. Final UI and interrupted-device acceptance remain. |
| 5.6 | Partial | Real UDisks loop setup/read-only mount/read/unmount passes for each native partition-image format; writes return EROFS. Real FUSE catalog reads through mounted images now pass, with ENOMEDIUM before/after provider availability and ESTALE for lost timestamp precision. exFAT FUSE precision compatibility and concurrent-provider matrices remain. |
| 6.1 | Partial | Bounded raw-name catalogs exist; per-directory typed coverage/precision details remain incomplete. |
| 6.2 | Partial | Independent catalog/search rules exist and full backup/sync inclusion is preserved. Exact read-only lookup now loads old whole-drive/folder settings independently of status pagination; dark/light UI acceptance verifies unchanged saves retain both flags. CLI writes require both flags, omitted subtrees work, and successful rule jobs publish exclusions before completion. Operational-baseline accounting and rule-address alias/selector handling remain. |
| 6.3 | Partial | Search exclusion routing is reused by Shelf reference summaries, recursive photo discovery/features and audio constellation discovery/cache reads. Excluded roots are rejected before descent, with rechecks before decoding and publication. Isolated fixtures cover policy changes, cached results, symlink discovery aliases and explicit browsing/reads. Observed policy changes now retire Shelf summaries, photo similarity state and audio map discovery while retaining mutation owners. Broader search/answer/image-order caches, bind/case-fold/relocation aliases and the remaining consumer inventory still need verification. |
| 6.4 | Partial | Future scans honor subtree rules. Saved descendant records now have durable cleanup intents, mounted-reader retention, compaction, restart recovery and separate capture/redaction dates. Real FUSE and dark/light Files acceptance cover pending cleanup, busy readers and reopened Not catalogued boundaries. Broader alias/case-fold and concurrent-provider matrices remain with 6.3/6.8. |
| 6.5 | Partial | Hard bytes/entry/depth/time limits and million-file fixture pass; explicit larger-run UI and worker policy need evidence. |
| 6.6 | Partial | Real FUSE metadata/errors/RO tested; pagination, type fidelity and mutation errno matrix remain. |
| 6.7 | Partial | Provider metadata/digests now validate the same retained descriptor; same-size/mtime changed bytes are rejected when a digest exists, and a stale newer backup falls back to an older match. File backups also require their captured directory identity. Reuse of available sync/backup hashes and uncertain revision handling remain. |
| 6.8 | Partial | A real separate-process FUSE fixture proves an existing handle returns ESTALE after its provider changes while a new open selects a newer valid backup. Replaced file-backup directories and catalog self-mounts are rejected. Image-mount replacement, aliases and the broader concurrent-provider matrix remain. |
| 7.1 | Partial | Core guarded/source and drive tests pass; Old unbound operation records are protected, and atomic idempotent admission/restart/lost-owner fixtures pass. Completed-item cancellation and the other migration cases remain. |
| 7.2 | Partial | Daemon/reconnect/restart fixtures pass; mount-event loss and interruption state tests remain. |
| 7.3 | Partial | Commit `6b876b69` supplies a disposable KVM guest. Real FAT32/exFAT/NTFS format/write/unmount/remount probes pass without host mount privileges or attached media; failed-command and timeout probes are rejected. Eight application tests per filesystem now pass: union/conflicts, deletion policy, versioned file backups/manifests/timestamps, catalog exclusion versus full backup/sync inclusion, case/name handling, and applicable NTFS-stream/FAT32-size checks. Tests use whole-volume loop files with filesystem-UUID fallback; partitioned-device, read-only/full-media and mount-option matrices remain. |
| 7.4 | Partial | Real FUSE and Btrfs tests pass; authorized image-loop tests remain. |
| 7.5 | Partial | Dark/light private acceptance now includes RAM pending counts, drive confirmation, offline export and reconnect, alongside catalog/policy/backup review flows. Resource, broader coverage and interrupted-recovery UI flows remain. |
| 7.6 | Partial | Million-file, delta, sparse-active-backup, RAM byte/inode limits, bounded guest available-memory pressure, underlying-tmpfs exhaustion and acknowledged data/namespace/metadata edits across daemon death pass. A 10 TiB sparse RAM entry is rejected before hashing. Overflow delivery, concurrent pressure/admission and unacknowledged interrupted writes remain. |
| 7.7 | Partial | Prior worktree passed workspace tests, format, clippy and documented architecture review; rerun affected and final gates after remaining changes. |

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
