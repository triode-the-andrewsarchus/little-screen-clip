#!/bin/sh
# Exhaust only bounded tmpfs mounts inside the disposable guest.
set -eu
test "${LITTLE_CLIP_DRIVE_VM:-}" = 1
test -f /work/.owned-vm-fixture
test "$(cat /proc/sys/kernel/random/boot_id)" != "$LITTLE_CLIP_HOST_BOOT_ID"
test "$#" -gt 0
mkdir -p /work/ram-store /work/pressure
mount -t tmpfs -o size=512m,nr_inodes=8192,mode=700 little-clip-ram-tests /work/ram-store
mount -t tmpfs -o size=512m,mode=700 little-clip-pressure /work/pressure
export LITTLE_CLIP_DRIVE_RAM_TEST_BASE=/work/ram-store
if "$@"; then
    result=0
else
    result=$?
fi
echo "GUEST_RAM_RESULT=$result"
if [ "$result" = 0 ]; then
    umount /work/pressure
    umount /work/ram-store
fi
exit "$result"
