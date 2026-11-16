#!/bin/sh
# Only an isolated guest may create/mount these disposable filesystem fixtures.
set -eu
test "${LITTLE_CLIP_DRIVE_VM:-}" = 1
test -f /work/.owned-vm-fixture
test "$(cat /proc/sys/kernel/random/boot_id)" != "$LITTLE_CLIP_HOST_BOOT_ID"
mkdir -p /work/media
partitioned=0
if [ "${1:-}" = --partitioned ]; then
    test "${LITTLE_CLIP_DRIVE_TEST_AUTHORITY:-}" = 1
    partitioned=1
    shift
fi
overall=0
for filesystem in vfat exfat ntfs3; do
    if [ "$partitioned" = 1 ]; then
        /usr/bin/truncate -s 320M /work/$filesystem.img
        printf 'label: gpt\nstart=2048, size=524288, type=EBD0A0A2-B9E5-4433-87C0-68B6B72699C7\n' |
            /usr/bin/sfdisk /work/$filesystem.img
        loop=$(/usr/bin/losetup --find --show --partscan /work/$filesystem.img)
        media=${loop}p1
        /usr/bin/udevadm trigger --action=add --subsystem-match=block
        /usr/bin/udevadm settle --timeout=10
    else
        /usr/bin/truncate -s 256M /work/$filesystem.img
        loop=$(/usr/bin/losetup --find --show /work/$filesystem.img)
        media=$loop
    fi
    case $filesystem in
        vfat) /usr/bin/mkfs.fat -F 32 "$media" ;;
        exfat) /usr/bin/mkfs.exfat "$media" ;;
        ntfs3)
            formatter=$(command -v mkntfs)
            LD_LIBRARY_PATH=/opt/drive-tools/lib "$formatter" --force --quick "$media"
            ;;
    esac
    if [ "$partitioned" = 1 ]; then
        /usr/bin/udevadm trigger --action=change --subsystem-match=block
        /usr/bin/udevadm settle --timeout=10
    fi
    mount -t "$filesystem" "$media" /work/media
    echo owned-fixture > /work/media/probe.txt
    sync
    umount /work/media
    mount -t "$filesystem" "$media" /work/media
    test "$(cat /work/media/probe.txt)" = owned-fixture
    rm /work/media/probe.txt
    echo "GUEST_FILESYSTEM_READY=$filesystem"
    capacity=$(/usr/bin/blockdev --getsize64 "$media")
    filesystem_uuid=$(/usr/bin/blkid -p -s UUID -o value "$media")
    part_uuid=$(/usr/bin/lsblk -dnro PARTUUID "$media")
    disk_sequence=$(/usr/bin/blockdev --getdiskseq "$media")
    if [ "$#" -gt 0 ]; then
        if LITTLE_CLIP_DRIVE_TEST_SOURCE_DIR=/work/media \
           LITTLE_CLIP_DRIVE_TEST_DEVICE="$media" \
           LITTLE_CLIP_DRIVE_TEST_FILESYSTEM="$filesystem" \
           LITTLE_CLIP_DRIVE_TEST_CAPACITY="$capacity" \
           LITTLE_CLIP_DRIVE_TEST_UUID="$filesystem_uuid" \
           LITTLE_CLIP_DRIVE_TEST_PARTUUID="$part_uuid" \
           LITTLE_CLIP_DRIVE_TEST_DISK_SEQUENCE="$disk_sequence" "$@"; then
            result=0
        else
            result=$?
            overall=1
        fi
        echo "GUEST_FILESYSTEM_RESULT=$filesystem:$result"
        if [ "$result" != 0 ] && [ "${LITTLE_CLIP_DRIVE_TEST_AUTHORITY:-}" = 1 ]; then
            /usr/bin/tail -n 40 /work/authority/polkit.log /work/authority/udisks.log
        fi
    fi
    if /usr/bin/findmnt -rn -S "$media"; then
        /usr/bin/umount "$media"
    fi
    /usr/bin/losetup --detach "$loop"
    echo "GUEST_FILESYSTEM_COMPLETE=$filesystem"
done
exit "$overall"
