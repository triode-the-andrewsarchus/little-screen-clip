#!/bin/sh
# A persistent virtual disk belongs only to this two-boot acceptance fixture.
set -eu
test "${LITTLE_CLIP_DRIVE_VM:-}" = 1
test -f /work/.owned-vm-fixture
phase=$1
shift
case "$phase" in
    prepare) /usr/bin/mkfs.btrfs --force /dev/vda ;;
    recover) ;;
    *) exit 2 ;;
esac
mkdir -p /work/image-state
mount -t btrfs /dev/vda /work/image-state
if [ "$phase" = prepare ]; then
    /usr/bin/dbus-uuidgen > /work/image-state/machine-id
fi
cp /work/image-state/machine-id /etc/machine-id
export LITTLE_CLIP_DRIVE_TEST_REBOOT_PHASE=$phase
export LITTLE_CLIP_DRIVE_TEST_REBOOT_DIR=/work/image-state
/bin/sh scripts/drive_tools/vm_images.sh "$@"
sync
