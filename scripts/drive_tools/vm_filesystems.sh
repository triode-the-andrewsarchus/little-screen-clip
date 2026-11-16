#!/bin/sh
# Only an isolated guest may create/mount these disposable filesystem fixtures.
set -eu
test "${LITTLE_CLIP_DRIVE_VM:-}" = 1
test -f /work/.owned-vm-fixture
test "$(cat /proc/sys/kernel/random/boot_id)" != "$LITTLE_CLIP_HOST_BOOT_ID"
mkdir -p /work/media
for filesystem in vfat exfat ntfs3; do
    /usr/bin/truncate -s 256M /work/$filesystem.img
    loop=$(/usr/bin/losetup --find --show /work/$filesystem.img)
    case $filesystem in
        vfat) /usr/bin/mkfs.fat -F 32 "$loop" ;;
        exfat) /usr/bin/mkfs.exfat "$loop" ;;
        ntfs3)
            formatter=$(command -v mkntfs)
            LD_LIBRARY_PATH=/opt/drive-tools/lib "$formatter" --force --quick "$loop"
            ;;
    esac
    mount -t "$filesystem" "$loop" /work/media
    echo owned-fixture > /work/media/probe.txt
    sync
    umount /work/media
    mount -t "$filesystem" "$loop" /work/media
    test "$(cat /work/media/probe.txt)" = owned-fixture
    rm /work/media/probe.txt
    echo "GUEST_FILESYSTEM_READY=$filesystem"
    if [ "$#" -gt 0 ]; then
        LITTLE_CLIP_DRIVE_TEST_SOURCE_DIR=/work/media \
        LITTLE_CLIP_DRIVE_TEST_DEVICE="$loop" \
        LITTLE_CLIP_DRIVE_TEST_FILESYSTEM="$filesystem" "$@"
    fi
    umount /work/media
    /usr/bin/losetup --detach "$loop"
    echo "GUEST_FILESYSTEM_COMPLETE=$filesystem"
done
