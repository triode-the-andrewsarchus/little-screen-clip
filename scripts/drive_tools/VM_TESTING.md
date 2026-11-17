# Disposable removable-filesystem fixtures

`../portable_drive_vm.py` boots a disposable Linux guest for drive tests. Host code
and `/usr` are exported read-only; the guest has no network or host block devices.
Writable images, loop devices, mounts and daemon state live in guest RAM. The runner
uses no host mount or elevation operations.

Provide QEMU, a static BusyBox with mount/insmod/poweroff, and an installed kernel
with matching modules. The current module collector uses the host's `modprobe`
dependency inventory and zstd-compressed modules. KVM is the default; `--accel tcg`
selects software emulation explicitly. `--kernel-release` selects the module inventory
when the kernel image is outside its installed release directory. BusyBox from the
minimal mkinitcpio package omits mount and insmod; use a full static build.

The guest uses host `/usr` read-only for filesystem utilities. An optional `--tools`
path supplies another read-only **usr tree**, such as locally extracted, verified
`ntfsprogs`. A locally extracted QEMU can use its existing `LD_LIBRARY_PATH` and
`QEMU_MODULE_DIR`; `--firmware` supplies its firmware directory. These tools need not
be installed system-wide.

From the repository root:

```bash
python3 scripts/portable_drive_vm.py \
  --kernel /path/to/matching/vmlinuz \
  --busybox /path/to/static/busybox \
  --guest-script scripts/drive_tools/vm_filesystems.sh \
  --output-dir /tmp/portable-drive-filesystem-run
```

`vm_filesystems.sh` creates FAT32, exFAT and NTFS images, verifies a file across a
real unmount/remount, and detaches each loop device. NTFS uses the kernel `ntfs3`
driver. Required formatters are `mkfs.fat`, `mkfs.exfat` and `mkntfs`; the latter may
come from `--tools`. The script refuses ordinary host execution and checks that its
boot identity differs from the host observation captured by the runner.

Append `-- /absolute/path/to/test-binary ARGS...` to invoke a command for each
mounted filesystem. The repository is available read-only at its host absolute path.
Each invocation receives:

- `LITTLE_CLIP_DRIVE_TEST_SOURCE_DIR=/work/media`
- `LITTLE_CLIP_DRIVE_TEST_DEVICE` identifying the guest loop device
- `LITTLE_CLIP_DRIVE_TEST_FILESYSTEM` (`vfat`, `exfat`, or `ntfs3`)
- `LITTLE_CLIP_DRIVE_TEST_CAPACITY` and `LITTLE_CLIP_DRIVE_TEST_UUID`, observed from
  the owned guest block device
- Private XDG, temporary and drive-service state directories on the guest's
  `/work` tmpfs, which supplies filesystem identities for backup publication.

Each command status is recorded as `GUEST_FILESYSTEM_RESULT`; a failing command
marks the overall run failed while remaining filesystem cases still run. Mount or
format failures abort an unsafe fixture setup. Application tests must opt into these
inputs and preserve their ordinary fixture ownership checks. The bootstrap/remount probe alone does not verify backup, sync,
catalog, device authorization or filesystem-name handling in the application.

The output directory must be new. It retains the initramfs, exact QEMU command,
console log and JSON result. A zero QEMU exit alone is insufficient: the guest must
publish exactly one successful completion marker. Failed commands and deadlines
produce a failed report; a timed-out QEMU process is killed and reaped by its owner.
The default deadline is 120 seconds, configurable with `--timeout`.

Verified on this host:

- All three native format/write/unmount/remount probes passed:
  `/tmp/portable-drive-vm-harness-smoke/report.json`.
- An injected `/usr/bin/false` produced a failed report despite QEMU exiting zero:
  `/tmp/portable-drive-vm-harness-refusal/report.json`.
- An injected sleeping command reached the first mounted filesystem and exceeded
  its three-second deadline; the report retained the timeout and no success marker:
  `/tmp/portable-drive-vm-harness-timeout/report.json`.
- Shell syntax, Python compilation, and refusal before host fixture creation passed.

These establish a usable isolated test environment. The application-level removable
filesystem matrix remains a separate acceptance gate.

For bounded RAM pressure, use `vm_ram.sh` with 4096 MiB guest memory and a current
backend test binary:

```text
--guest-script scripts/drive_tools/vm_ram.sh --memory-mib 4096 \
  -- /absolute/backend-test-binary service::tests::guest_ram:: --ignored --test-threads=1 --nocapture \
  --skip service::tests::guest_ram::crash_owner_probe
```

This is a guest-script/argument selection for the runner above, alongside its
required QEMU/kernel/BusyBox inputs. Build the normal debug helper first with
`python3 scripts/build_drive_tools.py --profile debug`. The script and Rust fixture
both check the guest marker and distinct boot identity before pressure operations.
Two separate 512 MiB tmpfs mounts bound the image store and memory-pressure file;
the tests do not fill the host's `/tmp` or intentionally provoke the guest OOM killer.

The first case raises the fixture's configured minimum around observed
`MemAvailable`, writes a 256 MiB pressure file and checks refused admission while
an existing dirty RAM view remains writable. After releasing the pressure file it
explicitly restores the fixture's normal minimum and admits another copy.
`MemAvailable` is an estimate with a changing baseline; the test does not assume
freeing a file immediately restores its cold-start value or relax production limits.

The second case fills the backing mount while the RAM image still has over 64 MiB
logical headroom. Writes must report ENOSPC/EIO rather than falsely acknowledge
lost data. After space is released, daemon restart must retain the same image,
acknowledged edits and pending recovery state, then permit successful writeback.
`/tmp/portable-drive-vm-ram-pressure-io-negative/report.json` records the expected
failure against the old helper; `/tmp/portable-drive-vm-ram-pressure-patched/report.json`
records both passing cases with the cache-error propagation fix. These do not
establish unflushed-process-kill or physical power-loss guarantees.

The third case starts an owned test child containing the daemon service. Its parent
writes a new file, overwrites an existing file and writes an 8 MiB file in 4 KiB
chunks, each in a separate session. The parent keeps the external file open without
fsync while killing and reaping only its child. It waits for the retained image's
exclusive lease to retire, restarts against the same state and checks exact contents,
image identity, pending full-check state and successful portable writeback.
The internal `crash_owner_probe` entrypoint is skipped in the module-wide invocation
because the crash test supplies its private fixture and invokes it explicitly.
The original helper loses the new file in
`/tmp/portable-drive-vm-ram-crash-negative/report.json`; the write-acknowledgement
patch passes all three cases in `/tmp/portable-drive-vm-ram-crash-matrix/report.json`.
The fourth case uses the same owned-child lifecycle to independently check 13
namespace-only edits: empty-file creation, mkdir, unlink, rmdir, truncate, chmod,
mtime, hard links, symlinks, rename, chown and xattr addition/removal. The writer keeps
create/truncate descriptors open and performs no later data write or fsync. The
previous helper loses the empty file in
`/tmp/portable-drive-vm-ram-namespace-negative/report.json`.

The fifth case sets the immutable inode flag, requires a held writable descriptor's
truncate to report EPERM, kills the daemon and checks the original bytes and retained
flag. The previous helper falsely returns success for that refused truncate in
`/tmp/portable-drive-vm-ram-truncate-negative/report.json`.
`/tmp/portable-drive-vm-ram-namespace-accepted/report.json` records all five guest tests
passing with synchronous mutation completion and corrected truncate error reporting.
Death during an unacknowledged operation remains a separate check. None of these
tests implies persistence through host reboot or power loss.

The metadata handoff and tmpfs working-directory follow-up passed all three command
invocations in `/tmp/portable-drive-vm-native-bootstrap/report.json`. The updated
failure-continuation probe, `/tmp/portable-drive-vm-native-refusal-all/report.json`,
records failure and completed cleanup for all three filesystems before rejecting the
run. Native permission/link/timestamp probes are retained in
`/tmp/portable-drive-vm-native-capabilities/guest.log`; FAT32/exFAT can acknowledge a
chmod without changing their presented mode, so application fidelity tests must check
observed metadata rather than syscall success alone.


Application acceptance is in `clip-drives`' ignored
`sync_edge_tests::native_fs::` tests. Build with the file-manager/backend feature
graph, then pass the emitted absolute test-binary path and
`sync_edge_tests::native_fs:: --ignored --test-threads=1 --nocapture` after `--`.
The tests require the guest marker, a distinct boot identity, and matching pinned
mount/device/filesystem facts; they create only unique fixture subdirectories.

`/tmp/portable-drive-vm-native-app-final/report.json` records eight passing tests
per filesystem. Common cases cover union/conflicts with restored timestamps,
explicit deletion policy, file-backup versions/manifests/observed timestamps,
metadata-only catalogs and exclusions, case collisions, and a shared table of
literal file/directory names. Driver-specific checks cover NTFS3 alternate-stream
refusal and FAT32 oversized-file refusal. These whole-volume loop fixtures do not
establish partition identity, interactive authentication, ntfs-3g or every mount
option. The guest's default NTFS3 mount is case-sensitive; FAT32 and exFAT fold
ASCII case. A pass does not imply every requested Unix filename is representable:
the invariant is exact-name success or explicit refusal with originals retained.


For real device-authority tests, select `vm_images.sh` as the guest script. It
starts private guest D-Bus, udev, UDisks2 and PolicyKit services, then uses GPT
partition fixtures through `vm_filesystems.sh --partitioned`. Guest-only PolicyKit
rules allow root and deny the unprivileged fixture user. `/run`, `/etc` and `/dev`
are created inside the guest; the host bus and host block devices are never shared.
This exercises real service decisions, not the desktop's interactive password UI.
The native authority services must be installed in the read-only `/usr` tool tree.

Additional environment observations are `LITTLE_CLIP_DRIVE_TEST_PARTUUID` (empty
for whole-volume fixtures), `LITTLE_CLIP_DRIVE_TEST_DISK_SEQUENCE`, and, for the
partition/authority wrapper, `LITTLE_CLIP_DRIVE_TEST_AUTHORITY=1`. The wrapper
refreshes udev metadata after formatting so unprivileged inventory can use the
same observed UUIDs. Cleanup finds the source by device even if an application
remounted it at another directory. Failed application cases retain bounded guest
service diagnostics in the console log.

`/tmp/portable-drive-vm-partition-authority-smoke/report.json` proves all three
GPT partition format/mount/remount probes and production-format `lsblk` inventory
with PARTUUID, capacity and disk sequence. It does not by itself prove image
backup or authorized/denied `OpenForBackup` calls.


The `service::tests::native_images::` ignored cases exercise the production
inventory and authority paths. Build the current helper, then the test binary,
serially:

```bash
cargo build -j 2 -p clip-drives --bin clip-drive-mount
cargo test -j 2 -p clip-file-manager -p clip-drives --lib --no-run
```

Select `vm_images.sh`, allow 4096 MiB guest memory for retained image versions,
and append the following command after the runner's `--` (use absolute paths from
the completed build):

```text
/usr/bin/env LITTLE_CLIP_DRIVE_MOUNT_HELPER=/absolute/repo/target/debug/clip-drive-mount /absolute/backend-test-binary service::tests::native_images:: --ignored --test-threads=1 --nocapture
```

The parent test invokes a UID-65534 child with an exact test selector and requires
its explicit denial marker; a zero-test child run cannot pass. Guest root is
allowed by the private PolicyKit fixture, and the unprivileged user is denied.
The test also creates/replaces only an owned alias node in a temporary `/dev`
subdirectory, checking that the same rdev does not authorize a replacement inode.
It verifies a wrong kernel disk sequence against a real authorized descriptor.

`/tmp/portable-drive-vm-authorized-images-final/report.json` passed the initial
full authority/device/image flow for all three source formats: denied access,
busy unmount, exact full-partition hashes and sizes, read-only image mounting,
source mount restoration and preserved prior versions. Catalog-through-image
coverage is a follow-up run requiring the current helper. The source format may
be mounted through a different driver when UDisks restores it (this host selected
exFAT FUSE after an initial kernel exFAT mount); record actual mount facts and do
not infer driver coverage from a volume label.


The expanded run `/tmp/portable-drive-vm-image-catalog-final/report.json` uses the
current helper and verifies live FUSE listing, no-provider ENOMEDIUM, image-backed
payloads, provider retirement, catalog unmounting and image manifest publication.
The runner supplies `/bin/umount` as a symlink to util-linux's `/usr/bin/umount`;
BusyBox does not support the arguments fusermount3 uses.

The test fixes one file's fractional timestamp and another to a whole second.
On this host exFAT FUSE drops the fractional part, so the first entry correctly
returns ESTALE while the whole-second entry opens. FAT32 and NTFS3 provider reads
retain their comparable timestamps. The test reports precision loss explicitly;
a green run covers conservative refusal, not complete cross-driver compatibility.
That limitation remains in the completion audit.
