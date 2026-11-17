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

The RAM sidebar reports distinct observed paths awaiting reconciliation, including
renames and removals. These are queued observations rather than an exact count of
different file contents. Each session tracks at most 4096 names and 1 MiB of path
bytes. Event loss, exhausted tracking limits or restart shows **Full check
required** alongside any known pending count. Routine scans still reconcile the
complete tree without declaring previously covered observations uncertain.
Offline edits remain queued, including after daemon restart. The RAM entry's menu
shows the last confirmed drive sync; that timestamp advances only after successful
reconciliation and a filesystem flush. A sync acknowledges only observations it
covered, retaining newer edits. Counts are advisory; eject and discard still use
their independent validation and confirmation flows.

Source inventory, quota admission, formatting and initial mounting run on one retained preparation worker. Independent partitions can continue reconciling during preparation. Quota reloads and further RAM admissions wait for that worker, so they cannot invalidate its reservation. Cancellation retains ownership until preparation actually exits; an already running formatter finishes under its existing process limit before empty setup can be retired. The daemon journals the metadata inventory and private allocation before formatting, then publishes the RAM session and sync enrollment together. Restart cleans only unpublished, identity-checked setup artifacts; published RAM data is retained. Every image is filesystem-checked before exposing its view, including recovery from a crash before the first ready record was committed.

Each RAM view uses a fixed-size image on verified tmpfs, served by the separately packaged `fuse2fs` helper. This bounds allocated bytes and inodes even when an external editor writes to it. The image grows sparsely within its reservation; session admission reserves future growth across existing sessions. Default ceilings are 6 GB and two million inodes per session, 16 GB and four million inodes in total, and eight sessions. Small cards reserve less. Expanded contents and formatting overhead must fit as well. Memory admission additionally reserves each native helper’s 512 MiB address-space limit. Reload refuses quota reductions below retained session reservations.

The default RAM location is `/tmp`. If it is not a usable tmpfs, the daemon tries
`XDG_RUNTIME_DIR`, then `/dev/shm`. Selection checks the retained directory's
actual filesystem, read-only state and effective-user write/search permissions.
Read-only or inaccessible default candidates are skipped. An explicit override
other than `/tmp` must itself pass these checks; it never silently selects a
different directory. Each backing directory is private (0700), with a 0600 image.
Internal RAM images and views are excluded from indexing and managed backup/sync destinations.

Use the RAM entry's menu to flush, save a durable copy elsewhere, or explicitly discard it. A normal daemon shutdown flushes and retires its filesystem server; restart verifies the retained image and checks its filesystem before remounting. Existing open view handles must be reopened after a restart. SIGTERM and Ctrl+C drain accepted jobs before the storage owner exits. The launcher waits for that drain and retains lifecycle files if it times out, instead of forcing a kill. Its `LITTLE_CLIP_STOP_TIMEOUT_SECONDS` default is 60; zero waits without a deadline. The repository systemd unit uses an unlimited stop timeout and signals the main process first; reload an installed unit before relying on those settings. An unavailable or changed backing image is reported explicitly and is never treated as an empty folder. Initial population installs watches and performs a catch-up reconciliation before declaring the view ready.

Eject and partition imaging flush ready RAM mirrors even when their automatic
synchronization is paused. A failed flush stops before unmount and leaves RAM
contents, pending observations and the previous drive confirmation available for
retry or export. An incomplete initial RAM population must be resumed or explicitly
discarded before these operations proceed; save it elsewhere first if needed.
Paused ordinary folder pairs
retain their paused behavior. Eject preserves the RAM view; discarding it requires
explicit confirmation, including when the portable partition is already offline.

Saving elsewhere runs on a retained reconciliation worker and creates a new private output folder. Completed files remain there if cancelled or interrupted. A saved copy does not advance the removable media's common baseline or count as a media flush. Details reports the last successful export separately from media writeback, including whether the initial RAM population was complete. Unavailable and incomplete RAM views cannot be opened from the sidebar.

Initial population and an explicit resume perform a second full comparison before
publishing the ready state. This catches earlier files changed during copying,
including equal-size edits preserving modification times, and paths added after
the first inventory. Failure or cancellation preserves already installed files,
the initialized backing and pending work while keeping the initial copy incomplete.
After resolving the source error, **Synchronize** resumes and verifies it before
enabling browsing. Later edits continue through ordinary watch/catch-up processing;
the copy is not an atomic frozen snapshot of a continuously changing drive.

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

Before starting, Files shows the selected partition, exact partition capacity,
destination and available destination bytes. Image review includes the size of a
full independent image plus 64 MiB of working headroom. FAT32 destinations reject images above 4,294,967,295 bytes before unmounting the source, regardless of free space. Btrfs sharing and sparse
zero ranges can reduce actual allocation; shared-image admission requires that
working headroom and warns when a full independent image would not fit. Other
image destinations require the full estimate before unmount. File backup admission
checks working headroom but does not promise a byte-exact allocation estimate.
Space is checked again at execution; concurrent users can still exhaust it.
Admission also checks effective-user write/search permissions on the retained
destination handle and refuses read-only filesystems before requesting unmount.

Reviews retain their selected roots, expire after five minutes, and are limited to
sixteen pending selections. Replaced directories or changed partition observations
require a new review. Cancel and Escape release the selection without creating a
backup or saving a preference. Select an existing destination directory; reviewing
a missing path does not create it. **Update the backup directory setting for future backups** is
optional. A failed preference write stops the backup before copying or unmounting.
Changing preferences later cannot redirect a reviewed job, including a one-time
destination. Review pins are released before partition unmount.
Other pending reviews that overlap an explicitly ejected/imaged partition are
retired as well; they must be reviewed again after the mount changes.
Late replies from cancelled or superseded file-manager previews are retired without
replacing a newer selection. Review cleanup does not create a completion notice.

CLI clients can use `clipctl drives preview-backup DRIVE --image --destination DIR`,
then `clipctl drives start-backup TOKEN` with optional `--restore-mounted` and
`--remember-destination`. `discard-backup-review TOKEN` releases an unused review.
The direct `backup` command remains available with execution-time space checks.

Partitions without a PARTUUID use an explicitly disclosed `fs-FILESYSTEM_UUID` bucket. Unidentified media receive a fresh `unidentified-…` bucket for each explicit backup rather than sharing an invented identity.

Each completed version has a checksummed SQLite manifest retaining captured paths, hashes, source metadata and filesystem identity. Publication updates the current-version reference only after the version and manifest are durable. A prepared publication interrupted by a reference or registry failure is reconciled on restart or before the next backup; unresolved recovery issues appear in Drive jobs.

File backups copy ordinary files, hierarchy, available modes and timestamps. A writable source is collected with per-file fingerprints and revision checks rather than an atomic source-filesystem snapshot. Changed files get up to three collection attempts; a file that keeps changing stops publication and leaves the previous backup current. The manifest records the installed revisions, collection times and retried paths, including raw Unix names. Files can represent different moments, and paths added after enumeration may require another backup. A partition image captures the entire selected partition, including unused sectors. Imaging flushes enrolled changes and unmounts the partition before requesting exclusive read access. A busy partition or declined authentication leaves an actionable failed job.

File backups also capture exposed ownership, modes, access/modification times and
extended attributes, including POSIX ACLs. Restoration verifies the destination's
actual values; unsupported fields, timestamp rounding and permission failures
produce per-path omissions in the checksummed SQLite manifest. The job result
reports affected paths, omitted fields and paths whose source attributes could
not be completely observed. A completed payload backup can therefore carry
metadata warnings. Older backups have unknown fidelity, rather than an implied
clean report.

The optional `backup_metadata` table keys JSON facts by raw relative Unix path
bytes (an empty path represents the source root). It records raw attribute
names/values, source link counts, observed creation times, original access times
and typed omissions with errno when available. Source ownership, inode, device,
mode and change/modification times remain in the inventory records. Change and
creation times are historical observations; they are not recreated destination
timestamps. Access times use the initial inventory observation while the source
inode remains the same, before backup reads alter it. Symlink operations retain
the link inode and never restore metadata through its user-provided target.

Attribute collection is bounded to 1024 attributes and up to 1 MiB per path,
with 64 KiB name-list/value buffers. Incomplete observations are disclosed.
Unobserved attribute names do not authorize deletion of destination attributes.
Coarse or uncertain source timestamps require another attribute comparison
during collection. Btrfs metadata-only versions share unchanged payload blocks
and retain earlier versions' attributes. Captured hard-linked regular files and
symlinks share an inode in the completed backup when the destination supports
it. The manifest's optional, losslessly encoded `hard_link_to` identifies the
representative path. A changed revision between aliases stops publication and
retains the previous version. Destination link failures retain complete
independent payloads and disclose the lost relationship, including errno.
Uncaptured aliases are disclosed without following names outside the selected
tree. Alias groups share the first inventory observation of access time.

During collection, a later alias can reuse a matching entry already completed
in this backup. The daemon verifies its bytes and captured source facts before
installing the link, avoiding another payload write. Destinations that refuse
links receive complete independent copies with fidelity warnings. Btrfs
snapshots also share unchanged extents across versions.

For stronger consistency, select **Temporarily mount read-only for collection**
in a file-backup review. Close open files first. The daemon flushes enrolled
changes, unmounts the partition, requests a read-only mount through UDisks2, and
verifies both the mounted view and backing filesystem are read-only. It repeats
that verification throughout collection and before publication. Desktop
authentication may be required. A read-only bind view over writable storage
cannot satisfy this mode; ordinary live collection remains available through
such a view.

After collection or cancellation, the daemon retires its source readers before
restoring the original access mode. An originally read-only partition remains
read-only. A restoration failure preserves the completed backup and reports the
failure separately. After a daemon interruption, Jobs retains the last recorded
phase and observed collection mount for inspection; restart never replays mount
authority automatically. The mode requires an initially mounted partition and
does not preserve arbitrary mount-option overrides.

The CLI equivalents are `clipctl drives backup DRIVE --read-only` and
`clipctl drives start-backup TOKEN --read-only` for a file-review token.
`--read-only` cannot be combined with imaging or `--restore-mounted`.
These use distinct daemon commands so older daemons reject unsupported requests
instead of silently performing a live copy. The file-tree/SQLite backup format
is the same for either collection mode; the manifest records the enforced policy.

The source Drive record in each new manifest includes observed partition-layout
facts: partition number, start sector and checked byte offset, logical sector
size, type, table label/flags, and the immediate containing table's type and
identifier. Unknown values remain unknown, including older records and optional
inventory columns unavailable in older util-linux versions. Whole-volume,
encrypted-device and optical observations do not invent partition layouts. The
image still omits the parent disk's partition table. `lsblk` reports START in
fixed 512-byte sectors even with `--bytes`; this is separate from the reported
logical sector size. See the [upstream lsblk column definitions](https://kernel.googlesource.com/pub/scm/utils/util-linux/util-linux/+/refs/tags/v2.41.1/misc-utils/lsblk.c).

Eject and image preparation use a retained worker for folder flushes and unmount authorization. Other independent drives can continue synchronizing while authorization is pending. Completed flush status is durable before unmount begins. The coordinator releases conflicting watch/cache pins while retaining descriptor-free scheduling claims; unrelated observed subtrees remain watched. A busy external reader produces an error with the observed mount state. Cancelling remaining work waits for an already accepted unmount result: a completed eject is still reported as completed, and a cancelled image reports that the partition is unmounted without starting the copy. Unmount authority requests keep their actual reply lifetime, so shutdown may wait for the desktop authorization dialog to resolve.

Partition jobs retain their selected drive, destination and typed preparation phase. Image progress distinguishes copying from full verification. After restart, an unfinished job reports its last durable checkpoint and any confirmed unmount/completed-image facts. An interrupted authority request may already have taken effect; its message asks you to inspect current state. Restart does not replay mount actions automatically.

**Mount again after imaging** is an optional image-backup setting, off by default.
It restores a previously mounted partition only when this job confirmed its unmount,
including after cancellation or an imaging failure. Restoration remains owned work
until the mount request returns, and checks the original device lifetime before
requesting or accepting a restored mount. A changed or replaced partition is refused;
an originally read-only mount is requested read-only again. A completed backup is
retained if restoration fails, with a separate warning and recovery details in Jobs.
The CLI equivalent is `clipctl drives backup DRIVE --image --restore-mounted`.

File and image backups validate their selected destination against managed storage before preparation, including when it came from startup configuration. Prepared jobs retain their selected destination and its index exclusion while running; changing preferences does not retarget a prepared image job. Reviewed requests also retain their selection while queued. Direct `backup` requests resolve configuration when preparation begins.

Bulk collection uses the source and destination directory handles captured at admission. Replacing either pathname fails the job instead of switching its input or output to the replacement. Catalog collection also pins its selected state directory before creating its database.

Child lookups use Linux `openat2` (available since Linux 5.6), with
`RESOLVE_NO_XDEV` to refuse nested mounts before entering their metadata or
content handlers. The check covers same-filesystem bind mounts too, including
catalog files bound back into backup providers. Guarded content readers reopen
the retained child descriptor. Leaf symlinks remain inspectable metadata and
cannot authorize file-content reads. The original kernel `EXDEV` error is kept
in catalog coverage. See the [openat2 manual](https://man7.org/linux/man-pages/man2/openat2.2.html).
Unsupported kernels return a traversal error; there is no unchecked fallback.

Image admission retains a metadata-only pin to the selected device node before flushing or unmounting. Authorization must return that same block-device object and kernel disk sequence. Both image passes periodically recheck its lifetime, with a final check before publication. Device-path replacement fails the job and preserves the previous completed version. Disk sequences are transient observations, separate from persistent folder enrollment. Older `lsblk` versions can still supply browsing and sync inventory; imaging requires `DISK-SEQ` support and a kernel that supplies `BLKGETDISKSEQ`.

On Btrfs, subsequent file versions start from a writable snapshot of the previous version and become read-only when completed. Reflink staging and changed-range writes also share unchanged blocks in large changed files and images. This shares data between versions; it does not promise global deduplication across unrelated backup trees. Other destination filesystems retain independent full versions.

No completed versions are automatically discarded by default. Configure `backup_keep_versions`, then explicitly invoke pruning to remove older completed versions. Mounted image/catalog providers are retained. Incomplete versions have a distinct name and are never presented as completed backups.

Completed images can be mounted explicitly read-only from the backup submenu or CLI. These mounted images can supply contents to an offline catalog. Merely listing a catalog does not mount an image.

Image-provider admission records the exact mounted view in the local drive-state
SQLite database, alongside its backup record in one transaction. The observation
binds the backup/image path and UDisks loop owner to the root identity, namespace,
boot ID and kernel unique mount ID. A replaced mount or changed context cannot
supply catalog contents through an old record. Kernel unique mount IDs are
available since Linux 6.8 and are not reused during a boot; see
[statx](https://man7.org/linux/man-pages/man2/statx.2.html).
Missing older observations require explicitly unmounting/remounting the image
before it can serve catalog reads. Offline listings still work. Mount admission
fails and cleans up if the kernel cannot supply that identity.

If image-mount validation/publication fails after mounting, cleanup uses the
operation's actual mounted path and loop owner. It attempts to record those facts
for retry, still attempts cleanup if that write fails, and reports any cleanup
failure alongside the original error. Successful retirement clears the runtime
mount observation with the final backup state. These facts stay local; backup
manifest/public record formats and removable filesystems are unchanged.

## Folder synchronization

Selected source subtrees must be actual directories within the selected partition.
Links inside a tree remain references; a replacement link cannot redirect the tree
root elsewhere. Each pass checks root identity, overlap and managed-storage boundaries
again. Moving a paired folder inside its source or into managed backups suspends
synchronization while preserving its files.

Use **Keep in sync → Pair with a folder…**. Initial synchronization gives both folders the union of their files. Later reconciliation compares content fingerprints with the saved common baseline; modification time alone never decides which side wins.

Concurrent edits preserve portable/local variants on both sides and create a persistent conflict record. Variant names include their origin and a content suffix to avoid overwriting an earlier conflict. Choose the same original on both sides to resolve it; acknowledging a conflict keeps its variants. Directory/type conflicts preserve complete variants. A deletion concurrent with an edit restores the edit and reports the conflict.

Deletion propagation starts disabled: a missing file is restored from the other side. Enable deletions explicitly after the initial merge when you want confirmed removals to propagate. Missing media, read failures, excluded coverage, and incomplete scans never count as deletions.

Close/write/rename notifications reconcile changed files promptly, with periodic inventories to recover missed events. Unchanged file fingerprints are reused only on known precise filesystems with validated live roots and complete watch coverage. Reconnection and daemon restart force fresh comparisons. Four bounded reconciliation slots keep full scans and initial RAM population off the coordinator. Work on the same partition or overlapping destination waits for its current writer to finish; independent pairs can reconcile concurrently. A separate bounded bulk worker keeps large file backups from blocking another partition’s RAM writeback. Change batches coalesce while their pair is busy. Watch registration advances incrementally and requests full coverage while incomplete.

Event queue overflow rebuilds directory watches as well as comparing contents,
so directories created during lost events regain prompt notifications. Exhausted
watch quotas retry at the configured synchronization interval. If inotify itself
is unavailable, synchronization uses periodic fresh comparisons and retries
watcher creation at that interval. Explicit synchronization remains available.

The daemon validates partition identity and additional device evidence before writing. Ambiguous cloned UUIDs suspend automatic synchronization. Local-folder replacement also requires re-enrollment rather than silently directing writes into a different directory.

## Offline catalogs and exclusions

**Save / update catalog…** stores hierarchy, names, sizes, types, available modification times, and coverage status without copying file contents. **Mount saved catalog** exposes a read-only ordinary filesystem view. Saved catalogs remain accessible from the sidebar after the original drive is removed.

Saving opens a review with entry, database-byte, depth and time limits. Edit the
four budgets or choose **Use larger limits** for this capture; **Use configured
limits** resets them to the current defaults. Canceling the review starts no job.
The selected budgets are recorded with the catalog and do not change later
captures. Catalogs created by older versions have unknown capture budgets.
Captures share one retained background worker with file backups; independent
drive synchronization continues while a capture runs.

The CLI supports individual per-run overrides; omitted fields inherit the
daemon's configuration when the retained worker is launched:

```sh
clipctl drives catalog part:PARTUUID --max-entries 1000000 \
  --max-bytes 1073741824 --max-depth 128 --timeout-seconds 1800
```

Without flags, the existing configured-limit request is unchanged. Explicit
overrides use a distinct request that older daemons reject, so they cannot
silently ignore the requested budgets. Reaching a limit still produces disclosed
partial coverage rather than claiming a complete listing. Effective budgets are
local SQLite metadata; nothing is added to the removable filesystem.

New catalogs record direct-listing and descendant coverage for each directory.
A folder labeled **Partial catalog** has failed, excluded or unvisited descendant
metadata; a configured exclusion remains **Not catalogued**. A partial directory
listing returns I/O failure at the end of its known entries. Looking up an
uncaptured name in that directory also returns I/O failure, so missing metadata
cannot claim the file was absent. A completed empty directory still lists normally.

The read-only `user.little-clip.catalog.details` extended attribute exposes
bounded JSON coverage, typed failure stage/reason/errno, observed source
mount/namespace and modification-time storage resolution. Per-entry failures and
directory coverage also remain in the paginated SQLite catalog. Capture does not
read file payloads. Redaction removes descendant records and updates retained
ancestor coverage; it preserves the original capture date.

Captured timestamp seconds/nanoseconds are preserved exactly. Precision is a
separate source-driver fact, never a content-matching rule: FAT modification time
has two-second storage resolution, exFAT ten milliseconds, and kernel NTFS3 one
hundred nanoseconds. These follow the kernel's
[FAT timestamp conversion](https://raw.githubusercontent.com/torvalds/linux/v6.12/fs/fat/misc.c),
[exFAT timestamp conversion](https://raw.githubusercontent.com/torvalds/linux/v6.12/fs/exfat/misc.c)
and [NTFS3 timestamp conversion](https://raw.githubusercontent.com/torvalds/linux/v6.12/fs/ntfs3/ntfs_fs.h).
Other drivers, including FUSE adapters, remain explicitly unknown. A nanosecond
field or a filesystem label never proves finer resolution. Catalog details show
the source mount type independently of the drive label. Existing provider
revision/hash validation stays conservative.

The mounted view preserves signed fractional modification times, including dates
before 1970. Invalid stored nanoseconds or unrepresentable capture/redaction dates
return `EOVERFLOW` rather than replacing the original date with a capture date.
The helper uses the corrected upstream
[fuser time conversion](https://github.com/cberner/fuser/blob/v0.18.0/src/time.rs).
Its FUSE request loop remains single-threaded, with at most 128 open provider
handles and 1 MiB per read request. Large client reads are split by the kernel;
each request still validates its retained provider before and after reading.

Saved FIFOs, sockets and device nodes are inert metadata entries. Files shows
their original type and skips content-preview jobs. The mounted view represents
these entries as regular-file metadata placeholders whose opens return
`EOPNOTSUPP`; they cannot create live pipes, sockets or device access. The
original numeric mode/type, size, dates, owner and available device number remain
in the saved record. Special entries do not make a metadata-only catalog partial
merely because their payload was not read. Backup/sync collectors retain their
existing unsupported-special-file warnings.

The read-only `system.little-clip.catalog.entry` extended attribute exposes the
original entry record, capped at 64 KiB. Unix paths and link targets retain their
original bytes. Device numbers absent from older records remain unknown.
Use `lgetxattr` to inspect a saved symlink itself. The `system` namespace is used
because Linux rejects `user` attributes on symlinks before reaching FUSE; the
attribute is generated metadata and cannot be written. Safe relative symlinks
within the catalog, including links back to its root, remain navigable. Absolute,
escaping and uncaptured targets remain unavailable for traversal while their
original link text remains inspectable through this attribute. These choices
follow the kernel's [special-inode behavior](https://raw.githubusercontent.com/torvalds/linux/v6.12/fs/inode.c),
[FIFO open behavior](https://raw.githubusercontent.com/torvalds/linux/v6.12/fs/pipe.c)
and [xattr namespace checks](https://raw.githubusercontent.com/torvalds/linux/v6.12/fs/xattr.c).

Catalog directory entries retain their stored positive row ID plus two as a
stable inode and resume cookie. Nested `..` entries identify the captured parent;
the virtual root identifies itself. Invalid or overflowing stored IDs return an
overflow error, and negative directory offsets are rejected. Valid cookies extend
through the largest signed Linux seek offset, with normal end-of-directory
behavior after that entry.

New catalog databases use version 2 so older helpers reject their partial-listing
semantics. Updated helpers also read version-1 catalogs, whose missing precision
and directory details remain unknown. Ordinary drive state and backup manifests
retain version 1. Keep the helper and applications from the same build together.

Listing and stat operations work offline. Opening a regular file requires a matching mounted original or available backup provider; unavailable contents return a media error. Changed provider revisions return a stale-entry error. A metadata catalog is a dated observation, not a frozen copy of payload bytes. Some exFAT FUSE drivers drop fractional modification times: catalog entries captured through a more precise driver currently return a stale-entry error in that case. The read-only image can still be browsed directly. Cross-driver catalog compatibility remains unfinished. Symlink targets outside the captured tree remain metadata-only.

Capture can reuse SHA-256 fingerprints from completed file backups without
reading the source file contents. It prefers the explicit current version, then
visits completed history in reverse registration order until eligible files are
covered or the capture budgets are exhausted. This optional enrichment requires a
matching drive/evidence, source-root identity and full per-file revision, plus
recorded support for queried change timestamps during backup collection. The
current conservative eligibility is Linux 7 or later with an observed ext4,
Btrfs, XFS or tmpfs driver and nonzero fractional change time. This follows the
kernel's [multigrain timestamp model](https://docs.kernel.org/7.2/filesystems/multigrain-ts.html);
nanosecond storage precision alone does not qualify. FAT32, exFAT, NTFS, unknown
drivers, older kernels and legacy manifests do not import hashes by size/mtime.
Their ordinary metadata capture still works.

The backup manifest is checksummed and queried through one retained descriptor,
within the catalog byte/time budgets. Corrupt, replaced, oversized or unsupported
manifests leave the metadata catalog usable and roll back imported hashes.
The additional collection-provenance field is optional; existing manifest and
catalog versions are unchanged. Manifest verification shares one byte allowance
equal to this capture's database-byte budget, reserving the observed file length
before each checksum check, including unsuccessful checks. Explicit larger
capture limits also increase that allowance. A failed candidate rolls back only
its own imports, so an older valid candidate can fill gaps without replacing
verified current-version hashes. Registry/header/row decoding is bounded, and
history uses an indexed cursor without a history-wide sort or repeated catalog
scans. Metadata remains usable when optional enrichment cannot continue.
The full uncertain-provider policy remains unfinished.

Capture can also reuse an enrolled partition/subtree's portable sync scan under
the same conservative queried-change policy. The local operational SQLite state
stores a small versioned provenance record with the drive/evidence, raw subtree,
source birth identity, boot, namespace, unique mount ID and scan generation.
Eligible cached file rows carry that generation; publication commits rows and
provenance together. These records and markers stay local and create no metadata
files on the removable medium. They extend the existing sync scan cache rather
than creating a second index.

Catalog enrichment checks both generation and full file revision, maps subtree
paths without losing raw names, and retains verified backup hashes in preference
to sync candidates. Candidate/row decoding is bounded and uses one registry read
snapshot. Invalid, unavailable or oversized optional data leaves ordinary metadata
usable. Capture reads no source payloads to obtain these hashes. Fresh portable
observations from named syncs refresh their cached rows; incoming copies await a
new observation. Legacy or mismatched row generations must be rehashed before
publication can attach new provenance. Directory-change fallbacks take fresh full
comparisons, and coarse/unknown source filesystems cannot reuse a whole scan.
FAT32, exFAT and NTFS retain metadata-only behavior for this enrichment.

File-backup providers must retain their saved directory identity; replacing a
backup directory does not authorize the replacement to supply catalog contents.
Older records without that identity cannot supply file contents. When a catalog
entry has a digest, validation hashes the same retained descriptor used for reads.
A stale candidate can fall back to another matching backup, while an already open
file keeps its chosen provider and reports stale data if that provider changes.

The reloadable `drives.catalog_workers` setting allows one to four independent
catalog captures at once, defaulting to one. Raising it admits queued independent
captures; lowering it lets accepted work finish before admitting more. Captures
of the same partition or overlapping source wait for one another. File/image
backups remain exclusive with catalog captures, and metadata cleanup waits for
live captures to release ownership. The review displays the configured capacity;
its larger-run button changes only that capture's metadata budgets.

Catalog creation journals private staging ownership before writing metadata.
Cancelled unfinished captures are removed; interrupted captures are recovered by
the daemon's metadata maintenance worker without needing the original drive.
A sealed capture is checksummed, installed and published after restart. Changed
or unrecognized staging artifacts are retained with recovery status rather than
deleted. A cancellation arriving after sealing can still finish publication and
returns the completed result. Previously published catalogs remain available.

Folder **Catalog / indexing policy…** settings distinguish metadata catalog admission from search/content indexing. Excluding a subtree stops traversal before enumerating its children. These exclusions do not omit files from a full backup or an enrolled folder synchronization. Sync still needs its own minimal comparison baseline.

Whole-drive and folder editors read the exact saved rule before displaying editable
settings, independently of the recent status page. A failed or cancelled lookup
cannot open an editor with default flags. A missing exact rule means there is no
explicit override for that subtree; parent exclusions still apply. Saving a rule
publishes the indexing exclusions before its job reports success.

`clipctl drives get-policy DRIVE_ID [RELATIVE_FOLDER]` reads the same exact rule
without creating a job. Omit the folder for the whole partition. The `policy`
write command requires both `--catalog true|false` and `--index true|false`, so an
omitted flag cannot reset the other setting. Whole-partition `pair` commands also
accept an omitted `--subtree`.

Catalog collection and published indexing policy load every saved rule through
bounded SQLite pages from one retained snapshot. The page size does not limit the
number of rules honored. Published paths are sorted and deduplicated. Background
index clients retain their one-MiB registry read limit: an oversized, unreadable or
malformed registry blocks background indexing rather than accepting a partial list.

The focused private desktop acceptance scenario uses `--policy-editor-only` with
`scripts/portable_drive_acceptance.py` in either appearance. It seeds rules older
than the first status page, reopens both editors and verifies that saving unchanged
settings retains both stored flags.

Files labels excluded catalog folders **Not catalogued**, while a captured empty
folder remains empty. Opening an excluded folder reports that its contents were
excluded from the saved catalog. Coverage labels do not change file identity or
authorize content access. Catalog directory inventories do not probe file payloads
to classify their formats.

Excluding catalog metadata also queues removal of the saved descendant records
from existing catalogs for that drive. Each mounted view retains its immutable
snapshot until unmounted; close open files if unmounting reports that the view is
busy. Cleanup survives daemon restarts and does not require the original medium.
Repeated pending exclusions coalesce. Existing exclusions from older versions are
enrolled once during startup.

The catalog keeps its original capture date, source modification dates and original
coverage warnings. Metadata removal adds a separate redaction date and a virtual
filesystem revision, so cached previews cannot stand in for the changed snapshot.
Re-enabling catalog admission does not recreate deleted metadata; capture a new
catalog to include it again. Backup payloads/manifests and enrolled synchronization
baselines remain independent of this optional catalog cleanup.

The saved-catalog menu shows the capture date, any later redaction date, pending
snapshot release and cleanup failures. Its policy dialog explains this before
saving an exclusion. The focused private acceptance scenario is available with
`scripts/portable_drive_acceptance.py --catalog-redaction-only --appearance dark`
(or `--appearance light`), using the same owned fixture setup described below.

Cleanup compacts the local catalog database on the bounded bulk worker after its
readers retire. Compaction can require temporary free disk space; failed cleanup
retains its intent for retry. SQLite can change implicit rowids during compaction,
which is why an existing mounted view keeps its reader lease until unmounted.
[SQLite VACUUM documentation](https://www.sqlite.org/lang_vacuum.html)

The focused private-desktop check covers this distinction and the wrapped
navigation error without repeating RAM population or backup setup:

```bash
python3 scripts/portable_drive_acceptance.py --appearance dark --catalog-boundary-only --output-dir /tmp/catalog-boundary-dark
python3 scripts/portable_drive_acceptance.py --appearance light --catalog-boundary-only --output-dir /tmp/catalog-boundary-light
```

Shelf reference summaries, recursive photo similarity discovery/features, and audio
constellation discovery/cache reads honor drive indexing exclusions. Explicit folder
browsing and individual-file opening remain available. Recursive photo discovery does
not follow child symlinks. Newly excluded observations are rejected before decoding or
publishing a new index snapshot. The periodic drive worker observes policy changes
and retires cached Shelf summaries, photo similarity state and audio map discovery.
Accepted saves/sends and collected items remain owned. This refresh does not yet cover
every search/answer/image-order cache.

Catalogs, RAM mirrors, and backup destinations do not automatically enroll in content search. The default catalog budget is 250,000 entries, 256 MiB, depth 64, and ten minutes. The database byte limit is enforced independently of the entry limit (minimum 128 KiB). A budget limit or read failure is disclosed as partial coverage; excluded directories are not represented as known-empty folders. Explicit metadata redaction preserves the catalog's capture date and discloses the later change.

Admitted backup destinations remain registered locally even when they are used only
once or a backup fails. Their index exclusion is published before collection starts,
and synchronization cannot enroll inside those managed roots after restart. Completed
versions from older service releases also supply their historical backup roots.

## Configuration

Add overrides to the existing application configuration. The backup destination can also be selected through the UI or CLI; leave it unset to retain the lazy prompt. Explicit selection saves only `drives.backup_directory` to the daemon's active configuration file, preserving comments and unrelated settings. It uses a guarded, revision-checked atomic replacement and flushes the file and containing directory. Starting the daemon or browsing drives does not create or rewrite the configuration. Failed preference writes leave the previous runtime destination selected and report the error.

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
catalog_workers = 1 # 1..4 concurrent captures on independent sources
sync_interval_seconds = 30
# backup_keep_versions = 3
```

Reload the application configuration through `clipctl reload`. Removing the backup-directory setting and reloading clears the selection; older SQLite preferences cannot restore it on restart. Drive enrollment, deletion decisions, baselines, operation results, conflicts, and catalogs are operational state under the XDG user-data directory in `little-screen-clip/drives`. Paths retain their original Unix bytes. Devices need no marker files or Git repositories.

The on-disk formats are ordinary file trees and raw partition `.img` files for
backup payloads, SQLite for per-version manifests and local operational/catalog
databases, and JSON for the current-version references alongside backups. RAM
sessions use a fixed-size ext4 image on verified tmpfs and expose its files through
FUSE; saving a RAM session elsewhere exports an ordinary file tree. Paired removable
folders contain ordinary synchronized files and any preserved conflict variants,
without service databases or Git metadata. Catalog databases contain metadata, not
file payloads. Btrfs snapshots/reflinks apply only at a compatible backup destination;
the removable filesystem does not need Btrfs support.

The standalone `scripts/test_drive_admission.py` acceptance test uses a private
runtime/socket, source and SQLite state with the test-support `drive_fixture`
example and current `clipctl`. Build those binaries before running
`python3 scripts/test_drive_admission.py -v`. It drops acknowledgements, exercises
concurrent retries and restarts the owned host; it never connects to the user daemon
or inspects real block devices.

## CLI

Commands return an accepted operation ID promptly. Inspect its completion through `status`; client disconnection does not revoke an accepted write. Substitute IDs from the JSON status output in these examples.

Files assigns a UUID to each mutation request and retries one lost transport reply
with the same identity and parameters. Server errors are not retried. The daemon
atomically stores the request fingerprint and operation record before delivery;
repeating that identity returns the original operation, including its completed,
failed or interrupted result after restart. It never starts the work again.

CLI mutation commands accept `--request-id UUID`, before or after their subcommand.
Use the same UUID and exact parameters to recover a lost acknowledgement. If a CLI
reply is lost, its error includes the generated UUID. A different request using an
existing UUID is refused. A new intended operation needs a new UUID; inspect and
reconcile an interrupted result before starting it. Status, backup review and
cancellation remain direct controls and do not accept this flag. Legacy drive IPC
requests remain supported; new clients use the `SubmitDrive` envelope.

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

An isolated KVM guest now passes native FAT32, exFAT and kernel NTFS3 sync, file-backup, catalog, timestamp, conflict and filename checks. Literal directory-entry checks caught silent filename normalization; the backend now refuses ambiguous NTFS3 colon paths and FAT32/exFAT trailing-dot/space paths while retaining originals. Whole-volume loop fixtures exercise filesystem-UUID fallback. Real guest GPT partition imaging now passes authorized/denied access, busy-source refusal, full-device hashes, read-only image mounts, retained earlier versions and source mount restoration. Desktop password-agent interaction, ntfs-3g and alternate mount options remain separate checks. No attached user partition was used. Source filesystems need no Btrfs, ZFS or Git metadata.

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
