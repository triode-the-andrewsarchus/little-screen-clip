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
| 1.6 | Partial | Pinned roots and overlap policy project bind subtrees into observed filesystem coordinates. Descriptor-free scheduling scopes now retain that observation after pins are retired; they cannot authorize payload access. Watch/cache retirement covers overlapping observed filesystem regions, including bind aliases, while retaining unrelated watches. Pure overlap/projection tests and a real normal FUSE unmount with a retained scope pass; a restarted native fixture also proves startup-configured file backups refuse private RAM backing. Real bind/case-fold aliases, remaining managed-root coverage, image destinations and device-node lifetime binding still need the complete matrix. |
| 2.1 | Partial | Persistent subtree pairs and baselines exist; strengthen fault-injection evidence for committing both sides. |
| 2.2 | Partial | Union/equality/conflict fixtures pass; real case-insensitive destination behavior remains unverified. |
| 2.3 | Partial | Watch overflow now clears the root observation epoch; coarse/unknown filesystems never reuse full-scan fingerprints. Direct filesystem and loss/reconnect matrices remain. |
| 2.4 | Partial | Persistent collision-safe complete conflict variants tested; provenance and repeat behavior need a full matrix. |
| 2.5 | Partial | Deletion and delete-versus-edit tests exist; event-loss and cancellation matrices remain. |
| 2.6 | Partial | Type/link/size handling exists; explicit case/unsupported-name handling and real FAT/exFAT/NTFS checks remain. |
| 2.7 | Partial | Changed-file reconciliation, overflow invalidation and independent-drive scheduling tests pass. Coalesced path batches are shared and capped at 4096 paths/1 MiB; registration is incremental with 8192-watch/16 MiB/depth-256 budgets. Real overflow delivery matrices remain. |
| 3.1 | Partial | Capacity, MemAvailable and expanded contents are checked before admission; configurable per-session/aggregate byte, inode and session reservations now exist. Native quota and aggregate refusal tests pass; full pressure matrix remains. |
| 3.2 | Partial | Verified tmpfs path now falls back from the default /tmp to XDG_RUNTIME_DIR and /dev/shm. Explicit disk-backed override refusal is tested; fallback branch still needs an isolated non-tmpfs-/tmp acceptance environment. |
| 3.3 | Partial | Streaming initial population and incomplete state exist; a second reconciliation after installing watches catches earlier edits. Deterministic mid-population change/failure injection remains. |
| 3.4 | Partial | RAM sidebar and separate persisted RAM-save/media-flush state exist. Pending counts and final UI acceptance remain. |
| 3.5 | Partial | Removal/reconnect conflict fixture passes; interrupted media-write retention needs injected failure coverage. |
| 3.6 | Partial | Eject/flush/save/discard exist; prove waiting and dirty-session lifetime behavior. |
| 3.7 | Open | Fixed-size tmpfs images now enforce ongoing byte/inode growth, including external writers. Native ENOSPC for bytes/inodes, live-owner refusal, open-file graceful restart and daemon restart/removal tests pass. Forced exit after directory fsync and cancelled-initial-copy resume are tested. Unflushed-kill/underlying-tmpfs exhaustion/pressure matrix remains. |
| 4.1 | Partial | Lazy path prompt and pinned destination exist; picker cancellation/alias matrix needs explicit evidence. |
| 4.2 | Partial | Durable checksummed SQLite manifests and filesystem/registry current-version references now survive reference-installation faults; pruning removes matching manifests and protects current across clock changes. Space preflight remains. |
| 4.3 | Open | File copy preserves modes/times; bounded live-source retry, omission records and optional quiesced collection remain. |
| 4.4 | Partial | Real read-only Btrfs version snapshots and extent sharing passed; capability failure/admission matrix remains. |
| 4.5 | Partial | Real 4 KiB edits allocate 256 KiB, old bytes intact; strengthen final target verification evidence. |
| 4.6 | Partial | Explicit retention and mounted-provider refusal exist; open-provider/prune interruption matrix remains. |
| 5.1 | Partial | Preflush and unmount now run as retained stages. Real owned FUSE integration proves external-reader busy refusal, successful pin retirement, independent RAM writeback during a held unmount and retained completed eject after cancellation. Image cancellation after unmount prevents copy admission. Exact size/space presentation, device lifetime binding and real UDisks authorization coordination remain. |
| 5.2 | Host gate | UDisks OpenForBackup implemented; real authorized/declined/busy-device acceptance is blocked by interactive PolicyKit. |
| 5.3 | Open | Partition byte streaming now retains SHA-256 and durable source/backup manifests; typed layout facts and full authorized-device publication evidence remain. |
| 5.4 | Partial | Real Btrfs image delta and exact nonzero sectors pass; real non-Btrfs/device matrix remains. |
| 5.5 | Partial | No automatic remount is performed; explicit restoration option and typed recovery details need completion. |
| 5.6 | Host gate | Explicit read-only loop mount implemented; provider reads and recursion refusal need real image-loop acceptance. |
| 6.1 | Partial | Bounded raw-name catalogs exist; per-directory typed coverage/precision details remain incomplete. |
| 6.2 | Partial | Independent catalog/search rules exist, full backup/sync inclusion preserved; operational-baseline accounting remains. |
| 6.3 | Partial | Search exclusion routing and private UI checks exist; audit every recursive fallback consumer. |
| 6.4 | Open | Future scans honor subtree rules; removal of saved descendant records after readers release and explicit Not catalogued presentation remain. |
| 6.5 | Partial | Hard bytes/entry/depth/time limits and million-file fixture pass; explicit larger-run UI and worker policy need evidence. |
| 6.6 | Partial | Real FUSE metadata/errors/RO tested; pagination, type fidelity and mutation errno matrix remain. |
| 6.7 | Partial | Provider metadata/digests checked where captured; reuse available sync/backup hashes and uncertain revision handling need stronger evidence. |
| 6.8 | Partial | Pinned per-open files and self-mount refusal tested; alias, replacement and concurrent provider change matrix remains. |
| 7.1 | Partial | Core guarded/source and drive tests pass; migration/idempotent admission and completed-item cancellation tests still needed. |
| 7.2 | Partial | Daemon/reconnect/restart fixtures pass; mount-event loss and interruption state tests remain. |
| 7.3 | Host gate | Disposable FAT32/exFAT/NTFS partition matrix not run; requires authorized loop/mount facility, never attached user media. |
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
