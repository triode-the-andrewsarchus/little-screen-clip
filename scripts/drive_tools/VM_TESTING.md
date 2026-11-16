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
- Private XDG, temporary and drive-service state directories under `/work`.

Application tests must opt into these inputs and preserve their ordinary fixture
ownership checks. The bootstrap/remount probe alone does not verify backup, sync,
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
