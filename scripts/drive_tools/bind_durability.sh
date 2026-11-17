#!/bin/sh
# Standalone kernel/filesystem control; no little-clip process or system bus.
set -eu
test "${LITTLE_CLIP_DRIVE_VM:-}" = 1
test -f /work/.owned-vm-fixture
test "$(cat /proc/sys/kernel/random/boot_id)" != "$LITTLE_CLIP_HOST_BOOT_ID"
media=${LITTLE_CLIP_DRIVE_TEST_SOURCE_DIR:?}
device=${LITTLE_CLIP_DRIVE_TEST_DEVICE:?}
filesystem=${LITTLE_CLIP_DRIVE_TEST_FILESYSTEM:?}
test "$media" = /work/media
case "$device" in /dev/loop[0-9]*) ;; *) exit 2 ;; esac
case "$filesystem" in vfat|exfat|ntfs3) ;; *) exit 2 ;; esac
test -b "$device"
test "$(/usr/bin/findmnt -rn -o SOURCE -M "$media")" = "$device"
case "${1:-}" in ''|--file-fsync) ;; *) exit 2 ;; esac

work=/work/bind-durability-$filesystem
mkdir "$work"
bound=$media/owned-bind
payload=$media/durability-control.txt
test ! -e "$bound"
test ! -e "$payload"
mkdir "$bound" "$work/foreign" "$work/view"
echo "BIND_DURABILITY_KERNEL=$(cat /proc/sys/kernel/osrelease)"
echo "BIND_DURABILITY_FILESYSTEM=$filesystem"
/usr/bin/mount --internal-only --bind "$work/foreign" "$bound"
/usr/bin/umount --internal-only "$bound"
rmdir "$bound" "$work/foreign"
printf 'external application payload' > "$work/expected"
cp "$work/expected" "$payload"
/usr/bin/touch -m -t 202009131226.40 "$payload"
cmp "$work/expected" "$payload"
if [ "${1:-}" = --file-fsync ]; then
    /usr/bin/sync "$payload"
    echo BIND_DURABILITY_FILE_FSYNC=1
fi
/usr/bin/sync -f "$media"
/usr/bin/umount --internal-only "$media"
/usr/bin/mount --internal-only -t "$filesystem" -o ro,nosuid,nodev,noexec "$device" "$work/view"
result=0
cmp "$work/expected" "$work/view/durability-control.txt" || result=$?
if [ "$result" != 0 ]; then
    echo BIND_DURABILITY_OBSERVED_BYTES
    od -An -tx1 "$work/view/durability-control.txt"
fi
/usr/bin/umount --internal-only "$work/view"
echo "BIND_DURABILITY_RESULT=$filesystem:$result"
exit "$result"
