# RAM filesystem companion

`../build_drive_tools.py` builds the standalone e2fsprogs 1.47.4 `fuse2fs`
server. Its fixed-size ext4 image lives on verified tmpfs; the removable source
partition may still be FAT32, exFAT, NTFS or another supported ordinary filesystem.
The image supplies byte and inode limits for every client, including external editors.

The build fetches the official kernel.org release and checks the SHA-256 published
in that release's `sha256sums.asc`. `--source-archive` supports a previously downloaded
archive. Build dependencies include a C toolchain, make, pkg-config and libfuse3
development files. Runtime also requires `mke2fs`, `debugfs`, `e2fsck` (e2fsprogs),
libfuse3, fusermount3, libblkid and libuuid. Ordinary Cargo tests do not download or
build native tools. `launch.sh` and full workspace packaging include the companion.

The small GPL-2.0 patch implements `RENAME_NOREPLACE` while holding the server's
existing directory-mutation lock. Other rename flags remain rejected. The original
server rejects all nonzero rename flags, preventing the application's atomic
no-overwrite installations. The patch also makes directory/read-handle fsync flush
namespace metadata; the original skips flushing on handles opened read-only. A native
fault test creates a directory, fsyncs its parent, kills the server and checks the
recovered hierarchy. The unmodified fsync loses that directory; the patch preserves it.

The patch also propagates backing-I/O errors when the libext2fs write cache must
evict a dirty block. Upstream 1.47.4 stores that failure in a local variable but
returns the still-successful outer result. With a full backing tmpfs this can
acknowledge writes whose bytes never reach the image. The guest exhaustion test
fails against the old helper and passes with the error-propagation fix.

Each successful data write also flushes filesystem metadata and cached blocks into
the tmpfs image before acknowledgement. Flushing failures return an error. Previously,
a new file acknowledged to an external writer disappeared when the daemon and its
parent-bound helper were killed before that writer closed or fsynced the file.
The disposable guest crash test now recovers a new file, an overwritten file and an
8 MiB file written in 4 KiB chunks, then writes each back to the simulated source.
The external writer retains its open descriptor throughout the kill. This covers
acknowledged file data across process death. Successful namespace and metadata
mutations also flush through a shared completion helper under the existing lock.
Creation flushes before publishing its open handle, so a failed flush cannot leak
a handle FUSE will never release. The truncate callback now returns its operation's
error instead of the earlier successful lookup result; an immutable-file test proves
EPERM reaches the caller and the original bytes/flags survive daemon death.

A separate guest crash matrix checks empty-file creation (kept open), mkdir, unlink,
rmdir, truncate (kept open), chmod, mtime, hard links, symlinks, rename, chown and xattr
addition/removal, with no later data write or explicit fsync before the kill.
These tests do not cover host restart/power loss or a kill during an unacknowledged
operation. The image remains RAM-backed.

Every generated `drive-tools/` directory includes the executable, original source
archive, patch, upstream license notice, source hashes and a rebuild script. The
server is a separate process and is not linked into the Rust application.

To run the native acceptance tests after building a debug companion:

```sh
python3 scripts/build_drive_tools.py --profile debug
cargo test -j 2 -p clip-drives --lib native_ram_quotas -- --ignored
cargo test -j 2 -p clip-drives --lib daemon_sync_and_ram_survive_client_closure_and_media_removal -- --ignored
```

The mid-write-removal fixture uses a separate, explicitly requested test helper:

```sh
python3 scripts/build_drive_tools.py --profile debug --test-io-gate
LITTLE_CLIP_TEST_RAM_GATE_HELPER="$PWD/target/debug/drive-tools-test/fuse2fs" \
  cargo test -j 2 -p clip-drives --lib removing_media_during_a_partial_write_retains_ram_and_queued_operation_results -- --ignored --nocapture
```

This build applies `fuse2fs-test-gate.patch` after the normal patch, and labels the
bundle **TEST ONLY**. Its private inherited socket arms one write, reports an
eight-byte little-endian stored position after at least 256 KiB, and waits at most
20 seconds. `R` resumes the write; another response returns EIO. The fixture keeps
metadata responsive while that one data request waits, detaches only its owned
FUSE mount and checks both failure and cancellation. Rust's gated constructor is
compiled only for tests; ordinary helpers do not contain the gate.

Normal and gated builds have distinct source/cache fingerprints and default output
directories. Test builds refuse the normal debug/release packaging paths, including
symlinked target directories. Sources are published to their build cache only after
all patches succeed; an interrupted patch is reapplied from the verified archive.
Both patch files and exact rebuild steps accompany the gated executable. Do not
install the test helper as the production companion.
