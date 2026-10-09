#!/usr/bin/env bash
# Build the pinned GPL-2.0-or-later Timelens CLI as a separate executable.
set -euo pipefail
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
helper_root="$repo_root/target/tools/timelens"
revision=988c9f4773b25916aa404801c9e0f468f8052321
patch_digest="$(sha256sum "$repo_root/scripts/timelens-stream.patch" | cut -d' ' -f1)"
version="$revision:$patch_digest"
mkdir -p "$helper_root"
cp "$repo_root/scripts/README.timelens.md" "$helper_root/README.md"
cp "$repo_root/scripts/timelens-stream.patch" "$helper_root/timelens-stream.patch"
if [ -x "$helper_root/timelens" ] && [ -f "$helper_root/source.tar.gz" ] && [ -f "$helper_root/LICENSE.md" ] && [ -f "$helper_root/GPL-2.0.txt" ] && [ -f "$helper_root/version" ] && [ "$(cat "$helper_root/version")" = "$version" ]; then
    exit 0
fi
pkg-config --exists gstreamer-1.0 gstreamer-app-1.0 || {
    echo 'Building Timelens requires the GStreamer and GStreamer App development packages.' >&2
    exit 1
}
build_source="$(mktemp -d "$helper_root/source.XXXXXXXX")"
trap 'rm -rf "$build_source"' EXIT
git -C "$build_source" init -q
git -C "$build_source" fetch -q --depth 1 https://github.com/timelens/timelens "$revision"
git -C "$build_source" checkout -q --detach FETCH_HEAD
git -C "$build_source" apply "$repo_root/scripts/timelens-stream.patch"
# Isolate this external CLI from the application's Cargo workspace.
printf '\n[workspace]\n' >> "$build_source/Cargo.toml"
cargo build --release --locked --manifest-path "$build_source/Cargo.toml" --target-dir "$helper_root/build"
# Preserve corresponding source and its GPL notice with the separately built binary.
cp "$repo_root/scripts/timelens-GPL-2.0.txt" "$build_source/GPL-2.0.txt"
cp "$repo_root/scripts/README.timelens.md" "$build_source/README.little-clip.md"
cp "$repo_root/scripts/timelens-stream.patch" "$build_source/little-clip.patch"
tar --exclude=.git --exclude=target -czf "$helper_root/source.tar.gz.new" -C "$build_source" .
mv "$helper_root/source.tar.gz.new" "$helper_root/source.tar.gz"
cp "$build_source/LICENSE.md" "$helper_root/LICENSE.md"
cp "$build_source/GPL-2.0.txt" "$helper_root/GPL-2.0.txt"
install -m 755 "$helper_root/build/release/timelens" "$helper_root/timelens.new"
mv "$helper_root/timelens.new" "$helper_root/timelens"
printf '%s\n' "$version" > "$helper_root/version"
