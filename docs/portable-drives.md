# Portable drives

The file manager's drive context menu controls RAM copies, versioned backups, automatic folder synchronization, and offline contents catalogs. The existing `little-clipd` daemon owns the work, so closing Files does not cancel an accepted copy or disable an enrolled sync pair. **Jobs…** in the Mounts section shows progress, failures, and preserved conflicts.

Build the daemon, CLI, file manager, and catalog helper together:

```sh
cargo build -j 2 -p little-clipd -p clipctl -p clip-file-manager -p clip-drives
python3 scripts/build_drive_tools.py --profile debug
```

Keep `clip-drive-mount` beside `little-clipd`. Catalog mounts use FUSE and `fusermount3`; partition imaging and read-only image mounts use UDisks2 with the desktop's PolicyKit authentication agent. Btrfs destinations also need the `btrfs` utility. Ordinary file backups do not require Btrfs on either side.

## RAM copies

**Copy to RAM** is offered for drives with reported capacity below the configured limit, initially 4 GB. At admission, the daemon checks for at least 8 GB available RAM and sufficient bytes/inodes on the selected tmpfs filesystem. The limits use decimal bytes. A small partition on a larger physical drive does not satisfy the default drive-capacity limit.

A completed copy appears as a separate **RAM** entry. Browse and edit that entry to commit locally first; the daemon reconciles those edits back to the connected partition. Ejecting or physically removing the partition leaves the RAM entry accessible. Reconnection compares both copies with the last common state before propagating edits.

Each RAM view uses a fixed-size image on verified tmpfs, served by the separately packaged `fuse2fs` helper. This bounds allocated bytes and inodes even when an external editor writes to it. The image grows sparsely within its reservation; session admission reserves future growth across existing sessions. Default ceilings are 6 GB and two million inodes per session, 16 GB and four million inodes in total, and eight sessions. Small cards reserve less. Expanded contents and formatting overhead must fit as well. Memory admission additionally reserves each native helper’s 512 MiB address-space limit. Reload refuses quota reductions below retained session reservations.

The default RAM location is `/tmp`. If it is not tmpfs, the daemon tries the verified tmpfs at `XDG_RUNTIME_DIR`, then `/dev/shm`. An explicitly configured disk-backed directory is refused. Internal RAM images and views are excluded from indexing and managed backup/sync destinations.

Use the RAM entry's menu to flush, save a durable copy elsewhere, or explicitly discard it. A normal daemon shutdown flushes and retires its filesystem server; restart verifies the retained image and checks its filesystem before remounting. Existing open view handles must be reopened after a restart. An unavailable or changed backing image is reported explicitly and is never treated as an empty folder. Initial population installs watches and performs a catch-up reconciliation before declaring the view ready.

Unsynchronized contents remain temporary: rebooting or losing tmpfs loses RAM-only data. Tmpfs can use swap. Legacy directory sessions remain recoverable, but must be saved/discarded before admitting another bounded session.

## Backups

Choose **Back up partition → As files…** or **As partition image…**. The destination prompt appears only when a backup is invoked without a configured directory. Completed versions are stored under:

```text
DriveBackupDir/
  PARTUUID/
    completed-file-version/
    completed-image-version.img
    VERSION_UUID.manifest.sqlite
    current-files.json
    current-image.json
```

Partitions without a PARTUUID use an explicitly disclosed `fs-FILESYSTEM_UUID` bucket. Unidentified media receive a fresh `unidentified-…` bucket for each explicit backup rather than sharing an invented identity.

Each completed version has a checksummed SQLite manifest retaining captured paths, hashes, source metadata and filesystem identity. Publication updates the current-version reference only after the version and manifest are durable. A prepared publication interrupted by a reference or registry failure is reconciled on restart or before the next backup; unresolved recovery issues appear in Drive jobs.

File backups copy ordinary files, hierarchy, available modes and timestamps. A writable source is collected with revision checks rather than an atomic source-filesystem snapshot. A partition image captures the entire selected partition, including unused sectors. Imaging flushes enrolled changes and unmounts the partition before requesting exclusive read access. A busy partition or declined authentication leaves an actionable failed job.

On Btrfs, subsequent file versions start from a writable snapshot of the previous version and become read-only when completed. Reflink staging and changed-range writes also share unchanged blocks in large changed files and images. This shares data between versions; it does not promise global deduplication across unrelated backup trees. Other destination filesystems retain independent full versions.

No completed versions are automatically discarded by default. Configure `backup_keep_versions`, then explicitly invoke pruning to remove older completed versions. Mounted image/catalog providers are retained. Incomplete versions have a distinct name and are never presented as completed backups.

Completed images can be mounted explicitly read-only from the backup submenu or CLI. These mounted images can supply contents to an offline catalog. Merely listing a catalog does not mount an image.

## Folder synchronization

Use **Keep in sync → Pair with a folder…**. Initial synchronization gives both folders the union of their files. Later reconciliation compares content fingerprints with the saved common baseline; modification time alone never decides which side wins.

Concurrent edits preserve portable/local variants on both sides and create a persistent conflict record. Variant names include their origin and a content suffix to avoid overwriting an earlier conflict. Choose the same original on both sides to resolve it; acknowledging a conflict keeps its variants. Directory/type conflicts preserve complete variants. A deletion concurrent with an edit restores the edit and reports the conflict.

Deletion propagation starts disabled: a missing file is restored from the other side. Enable deletions explicitly after the initial merge when you want confirmed removals to propagate. Missing media, read failures, excluded coverage, and incomplete scans never count as deletions.

Close/write/rename notifications reconcile changed files promptly, with periodic inventories to recover missed events. Unchanged file fingerprints are reused only within validated live root identities. Reconnection and daemon restart force fresh comparisons. A single bounded bulk worker keeps large backups from blocking another partition’s RAM writeback.

The daemon validates partition identity and additional device evidence before writing. Ambiguous cloned UUIDs suspend automatic synchronization. Local-folder replacement also requires re-enrollment rather than silently directing writes into a different directory.

## Offline catalogs and exclusions

**Save / update catalog…** stores hierarchy, names, sizes, types, available modification times, and coverage status without copying file contents. **Mount saved catalog** exposes a read-only ordinary filesystem view. Saved catalogs remain accessible from the sidebar after the original drive is removed.

Listing and stat operations work offline. Opening a regular file requires a matching mounted original or available backup provider; unavailable contents return a media error. Changed provider revisions return a stale-entry error. A metadata catalog is a dated observation, not a frozen copy of payload bytes. Symlink targets outside the captured tree remain metadata-only.

Folder **Catalog / indexing policy…** settings distinguish metadata catalog admission from search/content indexing. Excluding a subtree stops traversal before enumerating its children. These exclusions do not omit files from a full backup or an enrolled folder synchronization. Sync still needs its own minimal comparison baseline.

Catalogs, RAM mirrors, and backup destinations do not automatically enroll in content search. The default catalog budget is 250,000 entries, 256 MiB, depth 64, and ten minutes. The database byte limit is enforced independently of the entry limit (minimum 128 KiB). A budget limit or read failure is disclosed as partial coverage; excluded directories are not represented as known-empty folders. Existing catalogs remain dated versions when policy changes.

## Configuration

Add overrides to the existing application configuration. The backup destination can also be selected through the UI or CLI; leave it unset to retain the lazy prompt.

```toml
[drives]
# backup_directory = "/path/to/backups"
ram_capacity_limit = 4000000000
ram_min_available = 8000000000
ram_directory = "/tmp"
ram_session_max_bytes = 6000000000
ram_total_max_bytes = 16000000000
ram_session_max_entries = 2000000
ram_total_max_entries = 4000000
ram_max_sessions = 8
catalog_max_entries = 250000
catalog_max_bytes = 268435456
catalog_max_depth = 64
catalog_timeout_seconds = 600
sync_interval_seconds = 30
# backup_keep_versions = 3
```

Reload the application configuration through `clipctl reload`. Drive enrollment, baselines, operation results, conflicts, and catalogs live under the XDG user-data directory in `little-screen-clip/drives`. Paths retain their original Unix bytes. Devices need no marker files or Git repositories.

## CLI

Commands return an accepted operation ID promptly. Inspect its completion through `status`; client disconnection does not revoke an accepted write. Substitute IDs from the JSON status output in these examples.

```sh
clipctl drives status
clipctl drives status --offset 100
clipctl drives backup-directory /path/to/backups
clipctl drives ram DRIVE_ID
clipctl drives backup DRIVE_ID
clipctl drives backup DRIVE_ID --image
clipctl drives pair DRIVE_ID /local/folder --subtree optional/relative/folder
clipctl drives sync PAIR_ID
clipctl drives pause PAIR_ID
clipctl drives pause PAIR_ID --resume
clipctl drives deletions PAIR_ID --enabled true
clipctl drives catalog DRIVE_ID
clipctl drives mount-catalog CATALOG_ID
clipctl drives unmount-catalog CATALOG_ID
clipctl drives policy DRIVE_ID relative/folder --catalog false --index false
clipctl drives flush RAM_SESSION_ID
clipctl drives save-ram RAM_SESSION_ID /destination/folder
clipctl drives discard-ram RAM_SESSION_ID --yes
clipctl drives mount-image BACKUP_ID
clipctl drives unmount-image BACKUP_ID
clipctl drives prune DRIVE_ID
clipctl drives cancel OPERATION_ID
clipctl drives acknowledge CONFLICT_ID
```

## Validation

Backend tests use owned ordinary-file fixtures. Privileged/filesystem acceptance tests are explicit and use disposable fixtures:

```sh
cargo test -p clip-drives --lib
cargo test -p clip-drives --lib real_fuse_mount -- --ignored --nocapture
LITTLE_CLIP_DRIVE_TEST_BACKUP_DIR=/mnt/projects/backup_testing \
  cargo test -p clip-drives --lib btrfs_tests -- --ignored --nocapture
CLIP_DRIVE_CATALOG_FIXTURE=/mnt/projects/backup_testing \
  cargo test -p clip-drives --lib million_file_catalog -- --ignored --nocapture
```

UI automation uses the repository's private Weston/Niri runner, leaving the interactive desktop, clipboard and running daemon alone. Both dark and light acceptance runs passed RAM writeback, union enrollment, the lazy backup prompt, independent subtree exclusions, offline backup-backed FUSE browsing, reconnect conflict preservation and conflict review. Build the explicit owned-fixture helper before running it:

```sh
cargo build -j 2 -p clip-drives --features test-support --example drive_fixture --bin clip-drive-mount
python3 scripts/portable_drive_acceptance.py --appearance dark --output-dir /tmp/portable-drives-dark
python3 scripts/portable_drive_acceptance.py --appearance light --output-dir /tmp/portable-drives-light
```

The existing private-compositor runner requires its normal dependencies and English Tesseract data. Debug binaries were built for acceptance; the running desktop daemon was not restarted.

An isolated million-file fixture run measured 0.029 seconds for a catalog excluding its subtree and 0.102 seconds for a catalog capped at 1,000 entries. The capped database was 425,984 bytes and peak test-process RSS was 9,532 KiB. Fixture creation and removal were measured separately (11.47 seconds to create; 27.42 seconds total test time).

Real Btrfs tests changed 4 KiB in an 8 MiB file and an 8 MiB partition image. Each new version allocated 256 KiB exclusively and shared the remaining extents; old versions retained their original bytes. Real FUSE tests cover offline stat/listing, unavailable contents, exclusions, backup reads and rejected writes. A daemon test keeps a separate RAM mirror responsive while a 16 GiB sparse backup is active and verifies cooperative cancellation.

Loop-device FAT/exFAT/NTFS and interactive UDisks authentication remain host acceptance checks: this session has no noninteractive PolicyKit authorization or passwordless sudo. No attached user partition was used for testing. Source filesystems need no Btrfs, ZFS or Git metadata.

The complete workspace run passed **3,328 tests** across 50 suites, with 39 intentionally ignored host/fixture tests, using:

```sh
cargo test -j 2 --workspace -- --test-threads=2
cargo fmt --check
cargo clippy -j 2 --workspace --all-targets -- -D warnings
```

Use bounded test parallelism: the default test-thread count on this host saturated the existing shared studio preview queues and produced `QueueFull` failures. All 843 studio tests passed with two test threads. The three explicit drive fixture tests (Btrfs, FUSE and million-file catalogs) also passed separately. The final FUSE test includes rejecting a backup provider that points back at the serving catalog.

## Architecture audit

The backend depends on core and storage/runtime libraries, without egui or application-state dependencies. Core carries configuration/protocol models and existing guarded file primitives; the daemon owns the coordinator, Files owns menus/dialogs, and search consumes exclusion policy without owning drive state.

Compared with the pre-feature baseline, whole-repository duplication fell from 7.30% to 7.24%, average measured complexity changed from 3.44 to 3.47, maximum complexity stayed at 274, and dependency-cycle count stayed at 143. The checker still classifies the branch as a regression because it counts eight additional clone groups and flags the new guarded storage state machines. This is recorded rather than suppressed.

The review removed duplicated operation completion/progress/version checks, shared partition unmounting, reused scan observations for backup entries, split dialogs from menus, simplified positional streaming copies, and separated scan collection/descent from budget accounting (scanner complexity fell from 52 to 28). Remaining short similarities are entity construction, SQLite/FUSE adapters, guarded installation contracts and test fixtures; the backup/sync/publication branches retain their explicit error, identity, cancellation and durability checks. Existing browser action/context dispatch gained the required drive-policy action. Further generic builders or wrappers would obscure these contracts without removing a second implementation.
