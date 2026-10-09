#!/usr/bin/env bash
# Build the independent GPL-2.0-or-later FFmpeg helper as a separate process.
set -euo pipefail
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
helper_root="$repo_root/target/tools/video-strip"
source_root="$repo_root/scripts/video_strip"
source_path="$source_root/main.c"
if ! pkg-config --exists libavformat libavcodec libavutil libswscale; then
    echo 'FFmpeg development libraries unavailable; using video timeline fallback.' >&2
    exit 0
fi
mkdir -p "$helper_root"
cp "$source_path" "$helper_root/main.c"
cp "$source_root/average.cu" "$source_root/gpu_average.h" "$helper_root/"
cp "$source_root/README.md" "$helper_root/README.md"
cp "$repo_root/scripts/build_video_strip.sh" "$helper_root/build_video_strip.sh"
cp "$repo_root/scripts/timelens-GPL-2.0.txt" "$helper_root/GPL-2.0.txt"
cuda_compiler="$(command -v nvcc || true)"
if ! pkg-config --exists ffnvcodec; then
    cuda_compiler=""
fi
version="$(sha256sum "$source_path" "$source_root/average.cu" "$source_root/gpu_average.h" "$repo_root/scripts/build_video_strip.sh" "$source_root/rust_io/Cargo.toml" "$source_root/rust_io/Cargo.lock" "$source_root/rust_io/src/lib.rs" | sha256sum | cut -d' ' -f1):$cuda_compiler:$(pkg-config --modversion libavformat libavcodec libavutil libswscale | tr '\n' ':')"
if [ -x "$helper_root/video-strip" ] && [ -f "$helper_root/version" ] && [ "$(cat "$helper_root/version")" = "$version" ]; then
    exit 0
fi
read -r -a build_flags <<< "$(pkg-config --cflags --libs libavformat libavcodec libavutil libswscale)"
if [ -n "$cuda_compiler" ] && "$cuda_compiler" --ptx -O3 -arch=compute_75 "$source_root/average.cu" -o "$helper_root/average.ptx"; then
    python3 - "$helper_root" <<'PY'
import pathlib, sys
root = pathlib.Path(sys.argv[1])
data = root.joinpath('average.ptx').read_bytes() + b'\0'
root.joinpath('average_ptx.h').write_text('static const unsigned char average_ptx[] = {' +
                                       ','.join(map(str, data)) + '};\n')
PY
    read -r -a cuda_flags <<< "$(pkg-config --cflags ffnvcodec)"
    build_flags+=(-DVIDEO_GPU_AVERAGE -I"$helper_root" -ldl "${cuda_flags[@]}")
else
    echo 'GPU area reduction unavailable; retaining full-frame averaging.' >&2
fi
if [ "$(uname -s)" = Linux ]; then
    cargo build --release --locked --manifest-path "$source_root/rust_io/Cargo.toml" --target-dir "$helper_root/rust-target"
    mkdir -p "$helper_root/rust_io/src" "$helper_root/rust_io/.cargo"
    cp "$source_root/rust_io/Cargo.toml" "$source_root/rust_io/Cargo.lock" "$helper_root/rust_io/"
    cp "$source_root/rust_io/src/lib.rs" "$helper_root/rust_io/src/lib.rs"
    cargo vendor --locked --manifest-path "$source_root/rust_io/Cargo.toml" "$helper_root/rust_io/vendor" >/dev/null
    cat > "$helper_root/rust_io/.cargo/config.toml" <<'CONFIG'
[source.crates-io]
replace-with = "vendored-sources"
[source.vendored-sources]
directory = "vendor"
CONFIG
    build_flags+=(-DVIDEO_URING "$helper_root/rust-target/release/libvideo_strip_io.a" -ldl -lm)
fi
"${CC:-cc}" -O3 -std=c11 -Wall -Wextra -pthread "$source_path" -o "$helper_root/video-strip.new" "${build_flags[@]}"
mv "$helper_root/video-strip.new" "$helper_root/video-strip"
printf '%s\n' "$version" > "$helper_root/version"
