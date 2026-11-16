#!/usr/bin/env python3
"""Run owned drive fixtures in a guest with read-only host code/tools and no network.

Requires QEMU, KVM (or explicit --accel tcg), a kernel with matching installed
modules, and a static BusyBox including mount/insmod. This never mounts a host
filesystem or attaches host block devices. All writable media live in guest RAM.
"""
import argparse
import json
from pathlib import Path
import shlex
import shutil
import subprocess

MODULES = (
    'virtio_pci', 'virtio_blk', '9pnet_virtio', '9p', 'loop', 'vfat',
    'exfat', 'ntfs3', 'nls_cp437', 'nls_iso8859_1', 'nls_utf8', 'fuse',
)
APPLETS = ('sh', 'mount', 'mkdir', 'insmod', 'cat', 'poweroff')


def copy_modules(root, release):
    """Copy ordered module dependencies without loading anything on the host."""
    copied = set()
    names = []
    for module in MODULES:
        result = subprocess.run(
            ['modprobe', '--show-depends', '--set-version', release, module],
            check=True, capture_output=True, text=True,
        )
        for line in result.stdout.splitlines():
            if not line.startswith('insmod '):
                continue
            source = Path(line.split()[1])
            if source in copied:
                continue
            copied.add(source)
            name = source.name.removesuffix('.zst')
            target = root / 'modules' / name
            with target.open('wb') as output:
                subprocess.run(['zstd', '-d', '-c', str(source)], stdout=output, check=True)
            names.append('/modules/' + name)
    return names


def make_initrd(output, args, repo):
    root = output / 'root'
    for directory in ('bin', 'dev', 'proc', 'sys', 'tmp', 'modules', 'usr',
                      'opt/drive-tools', 'etc', 'work', str(repo).lstrip('/')):
        (root / directory).mkdir(parents=True, exist_ok=True)
    shutil.copy2(args.busybox, root / 'bin/busybox')
    for applet in APPLETS:
        (root / 'bin' / applet).symlink_to('busybox')
    # fusermount3 invokes this absolute path with util-linux-only options.
    (root / 'bin/umount').symlink_to('/usr/bin/umount')
    (root / 'lib').symlink_to('usr/lib')
    (root / 'lib64').symlink_to('usr/lib64')
    (root / 'etc/mtab').symlink_to('/proc/mounts')
    shutil.copyfile(args.guest_script, root / 'guest-test')
    modules = copy_modules(root, args.kernel_release or args.kernel.parent.name)
    host_boot = Path('/proc/sys/kernel/random/boot_id').read_text().strip()
    init = [
        '#!/bin/sh', '/bin/mount -t devtmpfs devtmpfs /dev',
        'exec </dev/console >/dev/console 2>&1', 'set -eu', 'export PATH=/bin',
        "trap 'result=$?; echo LITTLE_CLIP_VM_RESULT=$result; /bin/poweroff -f' EXIT",
        'mount -t proc proc /proc', 'mount -t sysfs sysfs /sys',
        *['insmod ' + shlex.quote(module) for module in modules],
        'mount -t 9p -o trans=virtio,version=9p2000.L,ro host_usr /usr',
        'mount -t 9p -o trans=virtio,version=9p2000.L,ro host_repo ' + shlex.quote(str(repo)),
    ]
    if args.tools:
        init.append('mount -t 9p -o trans=virtio,version=9p2000.L,ro host_tools /opt/drive-tools')
    init.extend([
        'export PATH=/opt/drive-tools/bin:/usr/bin:/bin', 'export LC_ALL=C',
        'mount -t tmpfs tmpfs /work',
        'mkdir -p /work/tmp /work/run /work/data /work/state',
        'export TMPDIR=/work/tmp XDG_RUNTIME_DIR=/work/run XDG_DATA_HOME=/work/data',
        'export LITTLE_CLIP_DRIVE_STATE_DIR=/work/state LITTLE_CLIP_DRIVE_VM=1',
        'export LITTLE_CLIP_HOST_BOOT_ID=' + shlex.quote(host_boot),
        'touch /work/.owned-vm-fixture', 'cd ' + shlex.quote(str(repo)),
        '/bin/sh /guest-test ' + shlex.join(args.guest_args),
    ])
    (root / 'init').write_text('\n'.join(init) + '\n')
    (root / 'init').chmod(0o755)
    archive = output / 'initramfs.cpio'
    listing = subprocess.Popen(['find', '.', '-print0'], cwd=root, stdout=subprocess.PIPE)
    try:
        with archive.open('wb') as destination:
            subprocess.run(['cpio', '--null', '--create', '--format=newc'], cwd=root,
                           stdin=listing.stdout, stdout=destination, stderr=subprocess.PIPE, check=True)
    finally:
        listing.stdout.close()
        listing.wait()
    return archive


def share(name, path):
    # QEMU's key/value parser escapes a literal comma by doubling it.
    path = str(path).replace(',', ',,')
    return ['-fsdev', f'local,id={name},path={path},security_model=none,readonly=on',
            '-device', f'virtio-9p-pci,fsdev={name},mount_tag=host_{name}']


def run_guest(args):
    repo = Path(__file__).resolve().parent.parent
    args.output_dir.mkdir(parents=True, exist_ok=False)
    archive = make_initrd(args.output_dir, args, repo)
    command = [
        args.qemu, '-machine', 'q35,accel=' + args.accel, '-cpu',
        'host' if args.accel == 'kvm' else 'max', '-m', str(args.memory_mib),
        '-nodefaults', '-nic', 'none', '-display', 'none', '-monitor', 'none',
        '-serial', 'stdio', '-no-reboot', '-kernel', str(args.kernel),
        '-initrd', str(archive), '-append', 'console=ttyS0 rdinit=/init panic=1',
        *share('usr', Path('/usr')), *share('repo', repo),
    ]
    if args.tools:
        command.extend(share('tools', args.tools))
    if args.firmware:
        command.extend(['-L', str(args.firmware)])
    (args.output_dir / 'command.json').write_text(json.dumps(command, indent=2) + '\n')
    log_path = args.output_dir / 'guest.log'
    timed_out = False
    with log_path.open('w') as log:
        try:
            result = subprocess.run(command, stdout=log, stderr=subprocess.STDOUT, timeout=args.timeout)
            return_code = result.returncode
        except subprocess.TimeoutExpired:
            timed_out = True
            return_code = None
    log = log_path.read_text(errors='replace')
    markers = [line.strip() for line in log.splitlines() if line.startswith('LITTLE_CLIP_VM_RESULT=')]
    success = not timed_out and return_code == 0 and markers == ['LITTLE_CLIP_VM_RESULT=0']
    report = dict(success=success, timed_out=timed_out, qemu_return_code=return_code,
                  guest_results=markers, log=str(log_path), guest_script=str(args.guest_script),
                  guest_args=args.guest_args)
    (args.output_dir / 'report.json').write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps(report, indent=2))
    return 0 if success else 1


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--qemu', default='qemu-system-x86_64')
    parser.add_argument('--kernel', type=Path, required=True)
    parser.add_argument('--kernel-release')
    parser.add_argument('--busybox', type=Path, required=True,
                        help='Static BusyBox including mount, insmod and poweroff')
    parser.add_argument('--firmware', type=Path)
    parser.add_argument('--tools', type=Path, help='Additional read-only usr tree for guest formatters')
    parser.add_argument('--guest-script', type=Path, required=True)
    parser.add_argument('--output-dir', type=Path, required=True)
    parser.add_argument('--accel', choices=['kvm', 'tcg'], default='kvm')
    parser.add_argument('--memory-mib', type=int, default=2048)
    parser.add_argument('--timeout', type=int, default=120)
    parser.add_argument('guest_args', nargs=argparse.REMAINDER)
    args = parser.parse_args()
    if args.guest_args[:1] == ['--']:
        args.guest_args.pop(0)
    for name in ('kernel', 'busybox', 'guest_script'):
        path = getattr(args, name).resolve(strict=True)
        setattr(args, name, path)
    args.output_dir = args.output_dir.absolute()
    if args.tools:
        args.tools = args.tools.resolve(strict=True)
    if args.memory_mib < 256 or args.timeout <= 0:
        parser.error('Guest memory must be at least 256 MiB and timeout must be positive')
    applets = subprocess.check_output([str(args.busybox), '--list'], text=True).splitlines()
    if not set(APPLETS).issubset(applets):
        parser.error('BusyBox is missing a required guest applet')
    return run_guest(args)


if __name__ == '__main__':
    raise SystemExit(main())
