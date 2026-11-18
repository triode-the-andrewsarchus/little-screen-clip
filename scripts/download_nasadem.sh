#!/usr/bin/env bash
# Download the complete NASADEM elevation set with the guarded application worker.
set -euo pipefail
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
if [[ -n "${LITTLE_CLIP_FILE_MANAGER:-}" ]]; then
    binary="$LITTLE_CLIP_FILE_MANAGER"
elif [[ -x "$repo_root/target/debug/clip-file-manager" ]]; then
    binary="$repo_root/target/debug/clip-file-manager"
else
    binary="$repo_root/target/release/clip-file-manager"
fi
cd "$repo_root"
exec "$binary" --download-nasadem "$@"
