# Portable-drive completion audit

The accepted plan remains the completion criterion. The previous goal turn made progress (implementation commits and test evidence), but did not prove every requirement. This audit treats unverified or incomplete items as unfinished. No item is marked complete solely because a broader test suite is green.

Audit starting revision: `7202c4ec`. Personal `.cargo/config.toml` and `config.toml` edits remain outside the task. Existing evidence is recorded in `docs/portable-drives.md`; further tests must match each requirement's scope. Host gates do not excuse independent implementation work.

| Plan item | Audit status | Current evidence / remaining work |
| --- | --- | --- |
| 1.1 | Partial | Numeric inventory/capacity and mount routing exist in discovery.rs; verify stable reconnect with changed device paths on disposable media. |
| 1.2 | Partial | UUID ambiguity refusal and fallback buckets exist; explicit fallback disclosure/enrollment needs end-to-end checks. |
| 1.3 | Partial | Reloadable DriveConfig exists; backup selection uses SQLite rather than the existing setting-preserving writer. |
| 1.4 | Partial | SQLite operations/baselines/conflicts and durable backup manifests now exist; ready-publication recovery is fault-tested. Copying-stage/per-item completion and coverage accounting remain. |
| 1.5 | Partial | Bounded admission, one bulk worker and retained operations exist; request idempotency and interrupted-operation reconciliation need work. Initial RAM population, Save RAM and periodic full sync still run on the owner thread and need independent-drive scheduling. Normal daemon signal handling now closes admission under the admission lock, cooperatively cancels queued/active work and joins the owner from a blocking context. Accepted-record completion and singleton release are tested. |
| 1.6 | Partial | Pinned roots and overlap policy exist; re-audit alias/managed-root coverage and image destinations. |
| 2.1 | Partial | Persistent subtree pairs and baselines exist; strengthen fault-injection evidence for committing both sides. |
| 2.2 | Partial | Union/equality/conflict fixtures pass; real case-insensitive destination behavior remains unverified. |
| 2.3 | Partial | Watch overflow now clears the root observation epoch; coarse/unknown filesystems never reuse full-scan fingerprints. Direct filesystem and loss/reconnect matrices remain. |
| 2.4 | Partial | Persistent collision-safe complete conflict variants tested; provenance and repeat behavior need a full matrix. |
| 2.5 | Partial | Deletion and delete-versus-edit tests exist; event-loss and cancellation matrices remain. |
| 2.6 | Partial | Type/link/size handling exists; explicit case/unsupported-name handling and real FAT/exFAT/NTFS checks remain. |
| 2.7 | Partial | Changed-file reconciliation and overflow invalidation tests pass; bounded coalescing and real overflow delivery matrices remain. |
| 3.1 | Partial | Capacity, MemAvailable and expanded contents are checked before admission; configurable per-session/aggregate byte, inode and session reservations now exist. Native quota and aggregate refusal tests pass; full pressure matrix remains. |
| 3.2 | Partial | Verified tmpfs path now falls back from the default /tmp to XDG_RUNTIME_DIR and /dev/shm. Explicit disk-backed override refusal is tested; fallback branch still needs an isolated non-tmpfs-/tmp acceptance environment. |
| 3.3 | Partial | Streaming initial population and incomplete state exist; a second reconciliation after installing watches catches earlier edits. Deterministic mid-population change/failure injection remains. |
| 3.4 | Open | RAM sidebar exists; separate pending counts and RAM-save/media-flush state are missing. |
| 3.5 | Partial | Removal/reconnect conflict fixture passes; interrupted media-write retention needs injected failure coverage. |
| 3.6 | Partial | Eject/flush/save/discard exist; prove waiting and dirty-session lifetime behavior. |
| 3.7 | Open | Fixed-size tmpfs images now enforce ongoing byte/inode growth, including external writers. Native ENOSPC for bytes/inodes, live-owner refusal, open-file graceful restart and daemon restart/removal tests pass. Forced exit after directory fsync and cancelled-initial-copy resume are tested. Unflushed-kill/underlying-tmpfs exhaustion/pressure matrix remains. |
| 4.1 | Partial | Lazy path prompt and pinned destination exist; picker cancellation/alias matrix needs explicit evidence. |
| 4.2 | Partial | Durable checksummed SQLite manifests and filesystem/registry current-version references now survive reference-installation faults; pruning removes matching manifests and protects current across clock changes. Space preflight remains. |
| 4.3 | Open | File copy preserves modes/times; bounded live-source retry, omission records and optional quiesced collection remain. |
| 4.4 | Partial | Real read-only Btrfs version snapshots and extent sharing passed; capability failure/admission matrix remains. |
| 4.5 | Partial | Real 4 KiB edits allocate 256 KiB, old bytes intact; strengthen final target verification evidence. |
| 4.6 | Partial | Explicit retention and mounted-provider refusal exist; open-provider/prune interruption matrix remains. |
| 5.1 | Open | Imaging flush/unmount exists; exact size/space estimate presentation and reader coordination need evidence. |
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
