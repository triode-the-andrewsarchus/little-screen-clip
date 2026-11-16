# Portable drive implementation plan

Provide RAM mirrors, partition backups, automatic two-way folder synchronization, and mountable offline catalogs through the existing daemon and file manager. Source media must work as ordinary FAT32, exFAT, NTFS, or other supported filesystems. Btrfs optimizations belong entirely to the backup destination.

Implementation branch: `feat/portable-drive-services`, created from local `main` at `bf6bf73e`. Preserve the pre-existing `.cargo/config.toml` and `config.toml` edits. This document describes planned work; no feature implementation or backup has run.

## Agreed behavior

| Capability | Behavior |
| --- | --- |
| Copy to RAM | Available below a configurable drive-capacity threshold, initially 4 GB, with at least 8 GB available RAM before admission. Copy files to actual tmpfs storage, browse and edit there, and promptly propagate edits to the connected drive. Removal leaves the RAM copy accessible. |
| Backup | Explicit full partition backup as an ordinary file tree or partition image, under `{DriveBackupDir}/{PARTUUID}`. Retain completed versions and optimize subsequent versions on Btrfs. |
| Automatic synchronization | Persist a folder/partition pairing. The existing daemon reconciles it whenever the drive is mounted, including while the file manager is closed. Initial synchronization preserves the union of files. |
| Conflicts and deletions | Compare both sides with the last successful common baseline. Preserve concurrent edits as `.portable` and `.local` variants on both sides, and notify the user. Deletion propagation requires an explicitly enabled policy after the initial merge. |
| Offline catalog | Save names, hierarchy, types, sizes, modification times, and available ordinary metadata without copying file payloads. Expose a read-only FUSE filesystem. Read contents from a validated mounted original or backup when available; otherwise return an error. |
| Backup configuration | Leave `DriveBackupDir` unset initially. Ask for it only when a backup is requested. For local testing, create and select `/mnt/projects/backup_testing/`. Do not commit that personal setting. |

## Architecture and reuse

Keep one new headless backend crate, provisionally `clip-drives`, shared by the daemon, file manager, and CLI. It owns the drive registry, persistent SQLite state, reconciliation, backups, RAM sessions, and catalog serving. It must not depend on egui, the workbench, or the file-manager application. Use an owned FUSE helper executable from the same package for mounted catalogs; a helper exit must not terminate the clipboard daemon.

Extend core's existing configuration and IPC schemas with drive preferences, operation requests, paginated status, and conflict decisions. Keep filesystem guards, lossless path encoding, identities, and common copy primitives in core; do not put a second search engine or a drive database in core. Add a small daemon adapter rather than expanding `main.rs` with operation implementations.

Reuse these existing mechanisms:

- `little-clip-core::{file_source, kernel_interface, file_inventory, path_address, path_serde}` for guarded reads, retained ordinary-directory handles, source revisions, mount/namespace identity, and raw path bytes.
- `jobs::task` and `process::CommandRequest` for bounded execution, owned child processes, and accepted operations whose completion outlives client requests.
- The guarded copy/staging/install logic in `clip-file-manager/src/file_manager/transfers/copy.rs` and mutation admission in `system_route.rs`. Extract only useful filesystem primitives into core, leaving browser batches, dialogs, and navigation with the file manager. Preserve their existing tests and safety behavior.
- The mount presentation in `file_manager/{mounts.rs,ui/mounts.rs}` and the existing udev stream's error/epoch handling. Share suitable event plumbing without changing the inspector's observation semantics. UDisks mount changes also need observation: insertion alone does not establish that a filesystem is mounted.
- `clip-search` enrollment/exclusion mechanisms, extended to understand drive-relative rules and managed virtual mounts without adding a search dependency to the daemon.

The current transfer implementation retains an entire source identity map and has a 200,000-entry safety preflight ceiling. A million-file drive needs streamed, descriptor-checked traversal and disk-backed manifests. Extracting primitives must not simply reuse that whole-tree allocation or remove the browser's existing guard limit.

An architecture baseline is recorded as `pre-portable-drive-services`. Compare against it during implementation and resolve accidental duplication, cycles, and complexity regressions.

## 1. Identity, configuration, and persistent operations

1. Discover numeric partition and parent-drive capacities, filesystem UUID, PARTUUID, filesystem type, mount identity, read-only state, and available device identity. Use stable identifiers rather than `/dev/sdX`, labels, or mount paths. RAM eligibility uses reported parent-drive capacity; the selected partition is the mirrored unit.
2. Associate a PARTUUID with additional device/filesystem evidence. Cloned cards may share UUIDs: ambiguity must suspend automatic writes instead of picking a device. Missing PARTUUID requires explicit enrollment with a clearly recorded fallback identity; do not invent a partition UUID or silently switch backup directories.
3. Add a reloadable drive configuration section: backup directory, RAM thresholds and resource limits, sync pairs, deletion policy, retention settings, and catalog/exclusion limits. Persist preferences using the existing setting-preserving writer. Use clear byte units in configuration and UI.
4. Store per-operation IDs, per-file baselines, conflict records, scan coverage, completed writes, backup versions, and pending work in a versioned SQLite database. Paths remain lossless bytes; timestamps retain source precision. Separate comparison fingerprints from inode identities, which may change after FAT/NTFS remounts. Flush completed destination writes and containing directories before recording a durable common baseline or backup completion.
5. Add a daemon coordinator with bounded read/copy/hash queues and serialized mutations per partition and destination. Requests return operation IDs promptly; status polling does not hold IPC connections open for a copy. Persist enough state to reconcile interrupted operations before retrying them.
6. Revalidate identities at execution and installation. Navigation, client disconnects, and daemon request timeouts must not discard accepted write results. Detect overlapping source/destination pairs and backup/sync recursion, including aliases and managed catalog/RAM mounts.

Delivery gate: fixtures demonstrate stable reconnection across device-path changes, refusal of ambiguous cloned identities, migration/reload behavior, idempotent admission, retained completion, and restart recovery.

## 2. Shared reconciliation and automatic folder sync

1. Enroll a mounted partition or a selected subtree against an ordinary local folder. Save a baseline for each pair and relative path only after the corresponding changes are successfully committed on both sides.
2. On first synchronization, merge both trees. Copy one-sided files to the other side. Equal contents converge without extra copies. Different contents at the same path preserve both variants, even if one timestamp is newer.
3. On later runs, compare each side against its saved baseline using content hashes and filesystem-aware metadata. Timestamps guide scheduling, not conflict resolution. Include correctness checks for unchanged size and coarse/preserved timestamps; inode values or newer wall-clock time alone never prove content equality.
4. For a concurrent edit to `report.txt`, preserve `report.portable.txt` and `report.local.txt` on both sides. Use collision-safe names if those names already exist, retain provenance, and make repeated reconciliation idempotent. Do not silently select an authoritative original. Surface unresolved conflicts in the daemon's persistent status and file-manager notifications.
5. Track deletion intent only from a previously complete baseline and successful enumeration. Missing/unmounted media, denied reads, policy exclusions, and truncated scans are never deletions. With deletion propagation disabled, restore one-sided missing files; when explicitly enabled, propagate confirmed deletions. Delete-versus-edit preserves the edit and reports a conflict.
6. Handle case-insensitive name collisions, FAT timestamp resolution, unsupported filenames, file/directory conflicts, symlinks, and FAT32 file-size limits explicitly. Never silently drop or rename user data to fit the other filesystem. Preserve unsupported metadata in manifests where useful and disclose fidelity limitations.
7. Observe close/write/rename events for prompt reconciliation; use throttled scans to recover missed events, watch-limit exhaustion, and overflow. On reconnection, compare both sides before replaying queued writes, since the drive may have changed elsewhere. Avoid a full-tree rescan for each edit.

Delivery gate: the daemon syncs with no file-manager process, preserves both edits after offline changes, handles interrupted copies and event loss, never propagates deletion from incomplete coverage, and reports all completed work after cooperative cancellation.

## 3. RAM mirrors with prompt writeback

1. Add **Copy to RAM** to eligible mounted-drive context menus. Check configured drive capacity and system `MemAvailable` before admission, plus tmpfs byte/inode headroom and aggregate active-session budgets. Unknown capacity or insufficient resources produces an actionable disabled reason.
2. Use a private directory on a verified tmpfs mount. Prefer `/tmp` where it is tmpfs; otherwise select a verified RAM-backed location. Check actual filesystem identity rather than assuming a pathname is RAM-backed. This system currently has tmpfs `/tmp`, approximately 40 GiB capacity, and approximately 62 GiB available RAM. Tmpfs can swap, so this is not a physical-RAM residency guarantee. [Kernel tmpfs documentation](https://docs.kernel.org/filesystems/tmpfs.html)
3. Copy the selected partition tree with progress and source validation. Publish **Browse RAM copy** when the initial copy has completed successfully; an interrupted copy remains explicitly incomplete. Detect changes during population and reconcile them before declaring it current. Do not block the UI or hold an entire drive in a byte buffer.
4. Expose a synthetic sidebar entry with drive identity, RAM path, connection state, and pending-write count. File-manager edits commit to RAM first. Queue writeback immediately after completed saves and closed external writes, using a short bounded coalescing interval. A RAM save and a confirmed media flush are distinct statuses.
5. Keep the RAM directory and edits when the drive is removed. On return, reconcile against the last common baseline before flushing. A removal during writeback retains pending RAM contents and reports the incomplete media write.
6. Distinguish **Eject portable drive**, **Flush pending changes**, **Save RAM copy elsewhere**, and **Discard RAM copy**. Normal eject waits for accepted writes or explains pending failures. Discarding unsynchronized RAM edits requires an explicit user decision. Do not age-evict dirty sessions.
7. Preserve owned RAM directories across file-manager closure and recover them after daemon restart where possible. Reboot or tmpfs loss still destroys RAM-only changes; offer an explicit durable save rather than silently turning RAM mode into a disk-backed cache. Continue enforcing growth/resource limits after admission.

Delivery gate: edit, disconnect, continue browsing/editing, reconnect with another portable edit, and converge through the normal conflict policy. Also test full tmpfs, inode exhaustion, daemon restart, and failed writeback without losing the RAM result.

## 4. Versioned filesystem backups and Btrfs deltas

1. Add **Back up partition → Files** for mounted ordinary partitions, including fixed drives. If `DriveBackupDir` is unset, open the destination picker only now. Canceling leaves configuration unchanged. Validate and retain the output directory, reject source/destination overlap, and create the test destination only during test setup.
2. Store backup state under `{DriveBackupDir}/{PARTUUID}/`, with completed version directories/subvolumes, a manifest, and a current-version reference updated only after successful publication. Incomplete work is never advertised as a completed backup. Keep historical completed versions until explicitly pruned; expose retention settings and a space check.
3. On ordinary destination filesystems, copy a complete version with the same guarded streaming mechanisms. Preserve available metadata and record any omissions. For writable live sources, retry changed files within a bounded budget and report that collection was not an atomic source snapshot; offer a quiesced read-only collection when stronger consistency is needed.
4. On Btrfs destinations, use a dedicated subvolume for each completed version. Start a private writable snapshot of the preceding completed version, then apply additions, changes, and removals to that new version. Leave prior completed versions immutable and publish the new version read-only. Check destination capabilities and permissions before admission; an unavailable optimization must be disclosed rather than silently advertised as incremental. This shares existing extents without requiring Btrfs on the source. [Btrfs subvolume documentation](https://btrfs.readthedocs.io/en/latest/Subvolumes.html)
5. For changed large files, clone the prior file into private staging and update only changed ranges before atomic installation. Use bounded comparisons, block fingerprints, and final verification so small changes do not rewrite and allocate an entire large file. A full-file replacement path would defeat this goal. Never patch a completed backup or rely on cross-version hardlinks for mutable files.
6. Treat Btrfs sharing as incremental version storage, not automatic deduplication of every independently created equal file. Verify allocated-space savings empirically. Retention removes complete unreferenced versions explicitly and never touches an active staging operation or a backup supplying an open catalog read.

Delivery gate: file equality and metadata verification on non-Btrfs destinations; on Btrfs, unchanged trees and a small edit to a large file consume predominantly metadata/changed extents, and old versions remain byte-for-byte unchanged. Interrupted publication preserves the previous current version.

## 5. Partition images and image deltas

1. Add **Back up partition → Image**, with exact partition size, space estimate, and destination. Coordinate with RAM writeback, sync, and file readers before requesting unmount; abort cleanly if busy or authorization is declined.
2. Obtain a read-only partition descriptor through UDisks2 `Block.OpenForBackup`, using desktop PolicyKit authentication where required. The API requires the device not already be in use. Avoid raw device paths in ordinary content readers and avoid making the full daemon privileged. [UDisks2 Block API](https://storaged.org/doc/udisks2-api/latest/gdbus-org.freedesktop.UDisks2.Block.html#gdbus-method-org-freedesktop-UDisks2-Block.OpenForBackup)
3. Stream the partition into an owned staging image, hash and verify it, then publish it atomically with identity, size, partition layout facts, filesystem information, and completion status. This is a partition image, including its filesystem and unused sectors, not an image of the entire physical disk.
4. For subsequent images on Btrfs, reflink the previous image and overwrite only changed verified blocks. Compare the full input as needed; without source change tracking this reduces destination allocation, not necessarily source read traffic. Non-Btrfs destinations retain complete independent images. Sparse zero ranges may reduce initial allocation where supported, without omitting nonzero unused sectors.
5. Remount only if this operation unmounted the partition and the user requested restoration of its mounted state. A failed image job retains the previous completed image and typed recovery status.
6. Allow completed images to be mounted read-only as backup providers for catalog reads. Never auto-mount an image merely to list catalog metadata. Block both image and file backup recursion into the source.

Delivery gate: synthetic partition images match byte-for-byte, denied authentication and busy devices leave no completed artifact, interruption retains the old backup, and Btrfs image updates preserve prior versions while allocating changed blocks.

## 6. Offline catalogs and no-index policy

1. Add **Save contents catalog** and **Mount saved catalog** for removable media. Capture metadata in a paginated SQLite hierarchy using raw names, numeric sizes, original timestamps/precision, file types, captured source identity, per-directory coverage, and typed failures. Do not read file payloads merely to inventory them.
2. Add persistent per-drive and drive-relative subtree rules with separate settings for **catalog metadata**, **search/content indexing**, and **backup/sync inclusion**. A no-index rule must not accidentally remove files from a full backup or exclude them from a configured sync pair. Explicitly enrolled sync pairs still need a minimal operational baseline for their included files; account for that storage separately from optional catalogs and search indexes. Keep rules locally so embedded media needs no marker files or repository metadata.
3. Default catalogs, RAM mirrors, and backup roots to no automatic content indexing. Honor explicit no-index rules in discovery, extraction, filename fallback, and recursive background features; opening an individual file remains possible. An exclusion must apply before descending into an excluded subtree, not after building its index.
4. Offer a folder action to exclude it from future catalog scans and remove any saved descendant catalog records after current readers release their snapshot. Display the boundary as **Not catalogued**, distinct from an empty folder. Historical catalogs remain explicitly dated versions rather than being silently rewritten.
5. Set configurable entry, database-byte, depth, time, and worker limits; proposed initial catalog limits are 250,000 entries, 256 MiB, depth 64, and 10 minutes. Stream bounded batches and expose partial/excluded/failed coverage. Allow an explicit larger run; never silently produce a supposedly full listing after hitting a limit.
6. Implement a read-only FUSE helper with stable inode IDs, paginated directory offsets, metadata operations, and descriptor-backed reads. Return an unavailable-media error for payloads with no provider; never fabricate zeros, pointer text, or an empty successful file. Mutation calls return a read-only error.
7. Resolve a captured entry first against a validated mounted original, then a matching completed backup. Check identity and revision, using captured content hashes when available from sync/backups. Metadata-only capture cannot prove exact byte equality; if a revision is uncertain or changed, expose it as stale and require refresh rather than pretending it matches the saved version.
8. Keep a chosen provider descriptor for each open file so a removal or provider change cannot splice bytes from different versions into one read. Exclude managed catalog mounts from automatic scans and drive synchronization. Prevent catalog-to-catalog resolution and deadlocks through the serving daemon's own FUSE path.

Delivery gate: list and stat an unplugged catalog from ordinary tools; read matching content from a mounted source, file backup, and read-only image backup; return clear unavailable/stale errors otherwise. A million-tiny-file fixture respects limits and subtree exclusions without unbounded RAM, repeated full scans, or hidden content decoding.

## 7. UI, CLI, packaging, and complete acceptance

Use the existing mount context menu for RAM copy, backup, catalog, and sync enrollment. Present progress, pause/cancel where meaningful, pending writeback, conflicts, last successful sync, partial coverage, and reconnect state. The backup-directory prompt appears only when backup is invoked with no destination; automatic work records an actionable pending decision instead of attempting to show a GUI from the daemon.

Add corresponding `clipctl` commands for listing drives/sessions, backup, RAM mirror management, sync configuration/status, conflict decisions, and catalog mount/unmount. Keep listings/status paginated. Update launch/install packaging for the backend/helper and document UDisks2, PolicyKit, FUSE, and optional Btrfs tooling/capabilities. Keep clipboard, USB display, and studio workers responsive during storage work.

Validation proceeds in this order:

1. Core guarded transfer/config/IPC tests and `clip-drives` planner/store/fault-injection tests, including non-UTF-8 names, identity replacement, unavailable sources, and cancellation after completed writes.
2. Daemon lifecycle tests with no file manager, mount-event loss, reconnection, restarted operations, and conflicts awaiting a future UI session.
3. Disposable FAT32, exFAT, and NTFS partitions against ordinary and Btrfs backup destinations. Include timestamp/case behavior, full/read-only media, partitions without UUIDs, cloned identifiers, and a FAT32-incompatible large local file.
4. FUSE integration tests on a host with `/dev/fuse`; privileged image/Btrfs tests use disposable loop devices and never arbitrary attached media. This execution environment currently lacks `/dev/fuse`, so real mount acceptance needs a capable host rather than being reported as passed here.
5. Private Weston/Niri UI acceptance for enrollment, lazy backup configuration, progress, conflict presentation, RAM browsing after removal, and catalog states. Use the repository's existing isolated compositor runner.
6. Meaningful scale checks: small embedded cards, a million tiny files, a large file with a small edit, a sparse image, watch overflow, database limits, memory pressure, and tmpfs inode exhaustion. Report elapsed time, peak RAM, bytes read/written, and Btrfs allocated bytes separately.
7. Run affected crate tests during each phase, then workspace tests, formatting, clippy, and architecture comparison before completion. Serialize Cargo commands and check free disk space as required by the repository.

## Tooling choice

Use UDisks2 for authorized partition access, Btrfs snapshot/reflink facilities for destination sharing, SQLite for transactional manifests, and the existing guarded file operations for copying. These choices leave embedded media as ordinary filesystems.

Git-annex remains an optional future archive backend, not a prerequisite for these drive operations. It supports normal tree exports/imports and offline content tracking, but introduces repository/content-state semantics that do not replace partition imaging, prompt RAM writeback, or the requested bounded metadata FUSE view. Its export documentation also requires care around independent writers. A native baseline and conflict policy is needed regardless, so adding annex to the initial implementation would add a second state owner without removing those responsibilities. [Directory remotes](https://git-annex.branchable.com/special_remotes/directory/), [tree export behavior](https://git-annex.branchable.com/git-annex-export/)
