# Portable-drive completion audit

The accepted plan remains the completion criterion. The previous goal turn made progress (implementation commits and test evidence), but did not prove every requirement. This audit treats unverified or incomplete items as unfinished. No item is marked complete solely because a broader test suite is green.

Audit starting revision: `7202c4ec`. Personal `.cargo/config.toml` and `config.toml` edits remain outside the task. Existing evidence is recorded in `docs/portable-drives.md`; further tests must match each requirement's scope. Host gates do not excuse independent implementation work.

| Plan item | Audit status | Current evidence / remaining work |
| --- | --- | --- |
| 1.1 | Partial | Numeric inventory/capacity and mount routing exist in discovery.rs; verify stable reconnect with changed device paths on disposable media. |
| 1.2 | Partial | UUID ambiguity refusal and fallback buckets exist; explicit fallback disclosure/enrollment needs end-to-end checks. |
| 1.3 | Partial | Reloadable DriveConfig exists; explicit backup selection now uses the existing setting-preserving writer with guarded atomic replacement, alongside operational SQLite state. Lazy selection, restart, raw paths, comments, symlinks, permissions and concurrent/replaced-file refusal pass targeted tests. The full quota/reload acceptance matrix remains. |
| 1.4 | Partial | SQLite operations/baselines/conflicts and durable backup manifests now exist; ready-publication recovery is fault-tested. RAM preflight/allocation journals and atomic session/pair enrollment now recover unpublished setup without deleting published data. Copying-stage/per-item completion and coverage accounting remain. |
| 1.5 | Partial | Bounded admission, one bulk worker and retained operations exist; request idempotency and interrupted-operation reconciliation need work. Full reconciliation, RAM data population and RAM export now use four retained worker slots, coalesced per-pair events and conflicting-drive/destination deferral. A 100 GB sparse initial sync and a separate 1 GB RAM population both leave another pair responsive. RAM metadata preflight, quota admission, formatting and initial mounting now use one retained preparation worker; image preflush and unmount authorization now use a retained partition worker. Source/destination pin admission, image/catalog mount controls, retention and startup work still need the full responsiveness matrix. Large-export responsiveness and startup-recovery scheduling need further evidence. Normal daemon signal handling now closes admission under the admission lock, cooperatively cancels queued/active work and joins the owner from a blocking context. Accepted-record completion and singleton release are tested. |
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
| 3.4 | Partial | RAM sidebar and separate persisted RAM-save/media-flush state exist. Pending counts and final UI acceptance remain. |
| 3.5 | Partial | Removal/reconnect conflict fixture passes; interrupted media-write retention needs injected failure coverage. |
| 3.6 | Partial | Eject/flush/save/discard exist; prove waiting and dirty-session lifetime behavior. |
| 3.7 | Open | Fixed-size tmpfs images now enforce ongoing byte/inode growth, including external writers. Native ENOSPC for bytes/inodes, live-owner refusal, open-file graceful restart and daemon restart/removal tests pass. Forced exit after directory fsync and cancelled-initial-copy resume are tested. Unflushed-kill/underlying-tmpfs exhaustion/pressure matrix remains. |
| 4.1 | Partial | Lazy selection produces a five-minute review bound to retained partition/destination roots. Cancel/expiry, replaced roots, changed partition facts, one-shot destination after preference changes and failed preference writes are backend-tested. Pending reviews are bounded and released before unmount. Dark/light focused UI runs prove cancellation, file backup publication, and image review; alias and broader failure matrices remain. |
| 4.2 | Partial | Durable checksummed SQLite manifests and filesystem/registry current-version references survive reference-installation faults; pruning removes matching manifests and protects current across clock changes. Admission/execution check working headroom; independent images require their full size estimate before unmount, while Btrfs permits shared allocation with explicit uncertainty. Detailed file-allocation estimation remains. |
| 4.3 | Open | File copy preserves modes/times; bounded live-source retry, omission records and optional quiesced collection remain. |
| 4.4 | Partial | Real read-only Btrfs version snapshots and extent sharing passed; capability failure/admission matrix remains. |
| 4.5 | Partial | Real 4 KiB edits allocate 256 KiB, old bytes intact; strengthen final target verification evidence. |
| 4.6 | Partial | Explicit retention and mounted-provider refusal exist; open-provider/prune interruption matrix remains. |
| 5.1 | Partial | Preflush and unmount run as retained stages. Real owned FUSE integration proves external-reader busy refusal, successful pin retirement, independent RAM writeback during a held unmount and retained completed eject after cancellation. Image cancellation after unmount prevents copy admission. Admission retains the device-node pin and checks its disk sequence across authorized image reads. Reviewed requests show exact capacity/destination/free bytes and bind the selected roots; non-Btrfs full-image space failure precedes unmount. Real guest UDisks/PolicyKit authorization, busy-source refusal and successful preflush/unmount/image/restoration now pass. Interactive password-agent and hotplug/replacement races remain. |
| 5.2 | Partial | Isolated real UDisks/PolicyKit tests now prove authorized image reads, unprivileged denial and busy-device refusal on FAT32/exFAT/NTFS partitions. The desktop authentication-agent/password interaction remains a separate UI gate. |
| 5.3 | Open | Partition byte streaming now retains SHA-256 and durable source/backup manifests; Full authorized-device hashes, exact partition image sizes and publication pass on real guest partitions; typed partition-layout facts remain. |
| 5.4 | Partial | Real Btrfs image delta and exact nonzero sectors pass; real non-Btrfs partition images now pass on FAT32/exFAT/NTFS with exact source SHA-256 and retained earlier images. Mount-option and interrupted-device matrices remain. |
| 5.5 | Partial | Optional mounted-state restoration now retains actual cleanup completion after imaging, failure or cancellation, refuses changed device lifetimes and reports separate restoration failures without losing completed backups. Owned authority and real FUSE fixtures pass; Real authorized image success and mounted-state restoration now pass on all three guest source formats. Final UI and interrupted-device acceptance remain. |
| 5.6 | Partial | Real UDisks loop setup/read-only mount/read/unmount passes for each native partition-image format; writes return EROFS. Real FUSE catalog reads through mounted images now pass, with ENOMEDIUM before/after provider availability and ESTALE for lost timestamp precision. exFAT FUSE precision compatibility and concurrent-provider matrices remain. |
| 6.1 | Partial | Bounded raw-name catalogs exist; per-directory typed coverage/precision details remain incomplete. |
| 6.2 | Partial | Independent catalog/search rules exist, full backup/sync inclusion preserved; operational-baseline accounting remains. |
| 6.3 | Partial | Search exclusion routing is reused by Shelf reference summaries, recursive photo discovery/features and audio constellation discovery/cache reads. Excluded roots are rejected before descent, with rechecks before decoding and publication. Isolated fixtures cover policy changes, cached results, symlink discovery aliases and explicit browsing/reads. Observed policy changes now retire Shelf summaries, photo similarity state and audio map discovery while retaining mutation owners. Broader search/answer/image-order caches, bind/case-fold/relocation aliases and the remaining consumer inventory still need verification. |
| 6.4 | Open | Future scans honor subtree rules; removal of saved descendant records after readers release and explicit Not catalogued presentation remain. |
| 6.5 | Partial | Hard bytes/entry/depth/time limits and million-file fixture pass; explicit larger-run UI and worker policy need evidence. |
| 6.6 | Partial | Real FUSE metadata/errors/RO tested; pagination, type fidelity and mutation errno matrix remain. |
| 6.7 | Partial | Provider metadata/digests checked where captured; reuse available sync/backup hashes and uncertain revision handling need stronger evidence. |
| 6.8 | Partial | Pinned per-open files and self-mount refusal tested; alias, replacement and concurrent provider change matrix remains. |
| 7.1 | Partial | Core guarded/source and drive tests pass; migration/idempotent admission and completed-item cancellation tests still needed. |
| 7.2 | Partial | Daemon/reconnect/restart fixtures pass; mount-event loss and interruption state tests remain. |
| 7.3 | Partial | Commit `6b876b69` supplies a disposable KVM guest. Real FAT32/exFAT/NTFS format/write/unmount/remount probes pass without host mount privileges or attached media; failed-command and timeout probes are rejected. Eight application tests per filesystem now pass: union/conflicts, deletion policy, versioned file backups/manifests/timestamps, catalog exclusion versus full backup/sync inclusion, case/name handling, and applicable NTFS-stream/FAT32-size checks. Tests use whole-volume loop files with filesystem-UUID fallback; partitioned-device, read-only/full-media and mount-option matrices remain. |
| 7.4 | Partial | Real FUSE and Btrfs tests pass; authorized image-loop tests remain. |
| 7.5 | Partial | Dark/light private acceptance passed; add pending-count/resource/coverage/recovery flows as completed. |
| 7.6 | Partial | Million-file, delta, sparse-active-backup and RAM byte/inode exhaustion tests pass. A 10 TiB sparse RAM entry is rejected before hashing. Overflow delivery, pressure and underlying-tmpfs exhaustion remain. |
| 7.7 | Partial | Prior worktree passed workspace tests, format, clippy and documented architecture review; rerun affected and final gates after remaining changes. |

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
