#!/bin/sh
# Bootstrap real guest authority services before creating owned partition fixtures.
set -eu
. scripts/drive_tools/vm_authority.sh
export LITTLE_CLIP_DRIVE_TEST_AUTHORITY=1
if [ "${1:-}" = --btrfs-destination ]; then
    shift
    /usr/bin/truncate -s 1G /work/image-backups.btrfs
    destination=$(/usr/bin/losetup --find --show /work/image-backups.btrfs)
    /usr/bin/mkfs.btrfs --force "$destination"
    mkdir -p /work/image-backups
    mount -t btrfs "$destination" /work/image-backups
    /usr/bin/btrfs subvolume create /work/image-backups/versions
    export LITTLE_CLIP_DRIVE_TEST_BACKUP_DIR=/work/image-backups/versions
fi
exec /bin/sh scripts/drive_tools/vm_filesystems.sh --partitioned "$@"
