#!/bin/sh
# Native application I/O only against the runner's owned emulated USB artifact.
set -eu
test "${LITTLE_CLIP_DRIVE_VM:-}" = 1
test -f /work/.owned-vm-fixture
test "$(cat /proc/sys/kernel/random/boot_id)" != "$LITTLE_CLIP_HOST_BOOT_ID"
filesystem=vfat
if [ "${1:-}" = --filesystem ]; then
    filesystem=$2
    shift 2
fi
test "$#" -gt 0
for attempt in $(/usr/bin/seq 1 200); do
    if [ -b /dev/sda ]; then break; fi
    /usr/bin/sleep 0.05
done
test -b /dev/sda
test "$(cat /sys/class/block/sda/removable)" = 1
case $filesystem in
    vfat) /usr/bin/mkfs.fat -F 32 /dev/sda ;;
    exfat) /usr/bin/mkfs.exfat /dev/sda ;;
    ntfs3)
        formatter=$(command -v mkntfs)
        LD_LIBRARY_PATH=/opt/drive-tools/lib "$formatter" --force --quick /dev/sda
        ;;
    *) exit 2 ;;
esac
mkdir -p /work/media
/usr/bin/mount --internal-only -t "$filesystem" /dev/sda /work/media
export LITTLE_CLIP_DRIVE_TEST_SOURCE_DIR=/work/media
export LITTLE_CLIP_DRIVE_TEST_DEVICE=/dev/sda
export LITTLE_CLIP_DRIVE_TEST_FILESYSTEM=$filesystem
export LITTLE_CLIP_DRIVE_TEST_CAPACITY=$(/usr/bin/blockdev --getsize64 /dev/sda)
export LITTLE_CLIP_DRIVE_TEST_UUID=$(/usr/bin/blkid -p -s UUID -o value /dev/sda)
if "$@"; then result=0; else result=$?; fi
if /usr/bin/findmnt -rn -M /work/media; then /usr/bin/umount --lazy /work/media; fi
echo "USB_WRITEBACK_RESULT=$result"
exit "$result"
