# Timelens helper and GPL compliance

[Timelens](https://github.com/timelens/timelens) is licensed under **GNU GPL version 2 or later (GPL-2.0-or-later)**. It is not an MIT-licensed dependency of Little Screen Clip.

The build script and patch beside this README deliberately keep Timelens as a **separate executable**, invoked by Files through command-line arguments and a pipe protocol. It is not linked into the application's Rust library or desktop binaries. This separation was chosen for license compliance reasons, to preserve the application's licensing boundary while using the GPL program. Modifications to Timelens remain GPL-2.0-or-later; the process boundary does not remove the helper's GPL obligations.

## Source and build

- `build_timelens.sh` downloads upstream commit `988c9f4773b25916aa404801c9e0f468f8052321` and applies `timelens-stream.patch`.
- The corresponding upstream source, `LICENSE.md` license, Cargo lockfile, and this patch describe the modified helper. The patch includes all local helper changes.
- The script retains the complete modified source as `target/tools/timelens/source.tar.gz` and copies this README, the patch, the upstream `LICENSE.md` notice, and the full `GPL-2.0.txt` license alongside the executable. Extracting that archive and running `cargo build --release --locked` reproduces the helper with the documented system dependencies.
- The script builds the helper separately into `target/tools/timelens/timelens`. Its version stamp includes the upstream revision and patch digest.
- `launch.sh build` and `launch.sh files` invoke this script. The first build requires GitHub access, Rust/Cargo, `pkg-config`, GStreamer and GStreamer App development packages. Runtime decoding needs codec plugins. NVIDIA acceleration additionally uses GStreamer NVCODEC/CUDA plugins when available; CPU decoding remains available.
- `LITTLE_CLIP_TIMELENS` can override the executable. The override must implement this patch's CLI/protocol extensions.

When distributing the helper binary, include its GPL license and make its complete corresponding modified source and build instructions available as required by that license. Keep upstream source and the applied patch together with release artifacts; do not assume the application's own license covers the helper.

`timelens-GPL-2.0.txt` contains the unmodified [GNU GPL v2 text](https://www.gnu.org/licenses/old-licenses/gpl-2.0.txt).

## Local extensions

The patch selects the requested video stream, correctly escapes file URIs, avoids unused thumbnail allocations, generates full-width timeline strips (ordinary frames for short files and one keyframe seek per output column for long files), and provides a persistent raw-frame scrub server. The normal upstream CLI is retained. `--fast` and `--preview-server` are private integration modes for Files, not a new linked API.


`--progressive PATH` additionally publishes atomic full-width PPM snapshots.
Short ordinary-frame timelines update about four times per second as frames
arrive, filling pending positions from their nearest completed column without
changing the final sampling. Long Timelens fallback snapshots arrive after each
worker chunk; the indexed FFmpeg helper normally handles long files and starts
with evenly distributed anchors. These extensions remain part of the GPL patch
and corresponding source archive. The FFmpeg AV1 streaming fallback has no
intermediate snapshot protocol.
