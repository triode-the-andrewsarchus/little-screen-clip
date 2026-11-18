#!/bin/sh
# Verify only the runner's owned emulated USB disk; never run on the host.
set -eu
test "${LITTLE_CLIP_DRIVE_VM:-}" = 1
test -f /work/.owned-vm-fixture
test "$(cat /proc/sys/kernel/random/boot_id)" != "$LITTLE_CLIP_HOST_BOOT_ID"
case "${1:-}" in
    ""|duplicate) ;;
    *) exit 2 ;;
esac
test "$#" -le 1

device() {
    for attempt in $(/usr/bin/seq 1 200); do
        found=
        for path in /sys/class/block/sd*; do
            if [ -f "$path/removable" ] && [ -b "/dev/${path##*/}" ]; then
                test "$(cat "$path/removable")" = 1
                test -z "$found"
                found=/dev/${path##*/}
            fi
        done
        if [ -n "$found" ]; then
            echo "$found"
            return
        fi
        /usr/bin/sleep 0.05
    done
    echo 'Owned USB disk did not appear' >&2
    return 1
}

original=$(device)
capacity=$(/usr/bin/blockdev --getsize64 "$original")
sequence=$(/usr/bin/blockdev --getdiskseq "$original")
echo "USB_PROBE_INITIAL_SEQUENCE=$sequence capacity=$capacity"
/usr/bin/mkfs.fat -F 32 "$original"
uuid=$(/usr/bin/blkid -p -s UUID -o value "$original")
mkdir -p /work/media
mount -t vfat "$original" /work/media
echo 'retained virtual USB payload' > /work/media/probe.txt
sync
/usr/bin/umount /work/media
for cycle in 1 2; do
    echo LITTLE_CLIP_USB_UNPLUG=1
    for attempt in $(/usr/bin/seq 1 200); do
        if [ ! -e "/sys/class/block/${original##*/}" ]; then break; fi
        /usr/bin/sleep 0.05
    done
    test ! -e "/sys/class/block/${original##*/}"
    echo "USB_PROBE_REMOVED=$original"
    if [ "${1:-}" = duplicate ]; then
        echo LITTLE_CLIP_USB_UNPLUG=1
        /usr/bin/sleep 2
        exit 0
    fi
    echo LITTLE_CLIP_USB_REPLUG=1
    returned=$(device)
    test "$(/usr/bin/blockdev --getsize64 "$returned")" = "$capacity"
    returned_sequence=$(/usr/bin/blockdev --getdiskseq "$returned")
    test "$returned_sequence" != "$sequence"
    test "$(/usr/bin/blkid -p -s UUID -o value "$returned")" = "$uuid"
    mount -t vfat "$returned" /work/media
    test "$(cat /work/media/probe.txt)" = 'retained virtual USB payload'
    /usr/bin/umount /work/media
    echo "USB_PROBE_RETURNED=$returned"
    echo "USB_PROBE_RETURNED_SEQUENCE=$returned_sequence capacity=$capacity"
    original=$returned
    sequence=$returned_sequence
done
