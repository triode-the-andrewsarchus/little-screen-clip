#!/bin/sh
# Bootstrap real guest authority services before creating owned partition fixtures.
set -eu
. scripts/drive_tools/vm_authority.sh
export LITTLE_CLIP_DRIVE_TEST_AUTHORITY=1
exec /bin/sh scripts/drive_tools/vm_filesystems.sh --partitioned "$@"
