#!/usr/bin/env bash
# Build the optional, standalone GPL-2.0-or-later logic decoder.
set -euo pipefail
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source_root="$repo_root/scripts/sigrok_decoder"
output="${1:-${CARGO_TARGET_DIR:-$repo_root/target}/tools/sigrok-decoder}"
if ! command -v pkg-config >/dev/null || ! pkg-config --exists libsigrokdecode; then
    echo 'libsigrokdecode development libraries unavailable. The optional Sigrok decoder was skipped.' >&2
    exit 0
fi
mkdir -p "$output"
read -r -a flags <<< "$(pkg-config --cflags --libs libsigrokdecode)"
temporary="$(mktemp "$output/clip-sigrok-decoder.XXXXXXXX")"
trap 'rm -f "$temporary"' EXIT
"${CC:-cc}" -O2 -std=c11 -Wall -Wextra -Werror "$source_root/main.c" -o "$temporary" "${flags[@]}"
cp "$source_root/main.c" "$source_root/README.md" "$output/"
cp "$repo_root/scripts/timelens-GPL-2.0.txt" "$output/GPL-2.0.txt"
chmod 755 "$temporary"
mv -f "$temporary" "$output/clip-sigrok-decoder"
