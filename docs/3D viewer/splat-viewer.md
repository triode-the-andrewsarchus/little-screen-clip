# Native 3D / Gaussian splat viewer

Research and application integration, 2026-10-03. Native previews for
`.ply`, `.splat`, `.sog`, `.glb`, and `.spz` are integrated into the big workbench
and little display. Navigation only; no editing.
Little-display playback uses a pre-rendered 30-frame orbit and releases scene
GPU allocations after baking.

## Renderer candidates

| Candidate | Relevant coverage | Integration assessment |
| --- | --- | --- |
| [wgpu-3dgs-viewer](https://github.com/LioQing/wgpu-3dgs-viewer) | Native Rust library; GPU projection, culling, depth sorting, spherical harmonics and blending; PLY and SPZ | Selected for the initial offscreen spike. The published 0.6 series uses wgpu 28, already present through Film Look. Does not provide SOG or Gaussian GLB loaders. MIT/Apache-2.0. |
| [Gauzilla](https://github.com/BladeTransformerLLC/gauzilla) | PLY/SPLAT, SPZ through the official C++ WASM module; WebGL and CPU sorting | Good web viewer, but browser/WASM and application coupling add adaptation work for a native headless renderer and egui viewport. MIT. |
| [wgpu-gs-viewer](https://github.com/abist-co-ltd/wgpu-gs-viewer) | Native/web wgpu rendering; PLY and SOG v2; global sort and optional tiled backend | Useful standalone Rust SOG decoder reference. Application-oriented, depends on winit and wgpu 29, and does not cover all requested containers. MIT. |
| [Spark](https://github.com/sparkjsdev/spark) | Three.js renderer; separate native Rust `spark-lib` decoders for ordinary/compressed PLY, SPLAT, SPZ v1–v3, bundled SOG v1/v2 | Its native SOG and compressed PLY decoders are integrated and GPU tested. MIT. |
| [PlayCanvas Engine](https://github.com/playcanvas/engine) / [SplatTransform](https://github.com/playcanvas/splat-transform) | Broad splat container coverage, including SOG and Gaussian GLB | Established JS/TS implementation and format references. Using the renderer directly would require embedding a browser; a Node conversion service would introduce a runtime dependency. Prefer native integration where feasible. MIT. |
| [bevy_gaussian_splatting](https://github.com/mosure/bevy_gaussian_splatting) | Bevy-integrated Gaussian rendering and format loading | Bringing a second application/rendering framework into the egui workbench is a larger integration than a direct rendering library. |

Repository source snapshots inspected locally:

- wgpu-3dgs-viewer: `2656e214eadb22522600071667281620e9e4632b` (development HEAD is 0.8 / wgpu 30; the spike uses published **0.6.0**).
- wgpu-gs-viewer: `b3dce83409bdf7276942f2154cb70749bad2561f`.
- Spark: `f5368253780bed4d863c96b1fdea9dd3d614d131`.
- SplatTransform: `435b972a97a278c655d8f7015b1355215cba0d71`.

Do not infer current format/version coverage solely from READMEs. The inspected
wgpu-3dgs-core and Spark SPZ readers explicitly accept only versions 1–3. A bounded
v4 ZSTD container adapter now reuses the upstream packed-attribute decoder,
following [Niantic’s SPZ format](https://github.com/nianticlabs/spz) at
`affd0ecea7fbb4c265ee119475af7ee5b2997482`.
Gaussian GLB uses extensions and must be distinguished from ordinary triangle
GLB; the existing Scenix mesh route does not prove Gaussian GLB support.

## Current implementation and evidence

`crates/little-clip-scene` contains a shared native offscreen adapter around
wgpu-3dgs-viewer. Dedicated splat extensions are connected to clipboard cards,
the workbench mirror, the read-only workbench viewer, and `--scene <PATH>` CLI
startup. GLB is classified on the worker: Gaussian data uses the splat renderer and
ordinary mesh data uses the existing mesh viewer. Little-screen GLB meshes also
use a finite cached orbit, reusing the existing card projection/shading code.

- Standard Gaussian PLY and SPZ v1–v3 load through existing upstream readers.
  SPZ v4 uses bounded ZSTD streams with the upstream v3 attribute decoder;
  positions, opacity, scales, rotations and SH retain their packed representation.
  Legacy headers are validated before decoding, and gzip checksums are checked.
  PlayCanvas compressed PLY uses Spark's native decoder with bounded file/element
  counts. SOG and compressed PLY share one adapter for Spark's Gaussian arrays.
- Gaussian GLB uses [OxideAV glTF](https://github.com/OxideAV/oxideav-gltf)
  0.0.4 for container parsing, typed JSON validation, strided/sparse accessors
  and integer normalization. No framework registry is enabled. Node instances
  preserve translation, rotation and positive nonuniform scale by decomposing
  transformed covariance with nalgebra. PlayCanvas's MIT-licensed SH rotation
  math is adapted with attribution/license to retain directional color under
  node rotation. Linear-color GLB receives sRGB encoding after composition.
  Unsupported kernels, mixed mesh/splat primitives, mixed color spaces,
  external buffers and non-decomposable node transforms report explicit errors.
  SPLAT uses its established 32-byte record representation; the renderer itself
  is not reimplemented.
- Bundled SOG v1/v2 uses Spark's native decoder, pinned to the inspected source
  revision with only SOG, PLY and Gaussian-array features enabled. ZIP expansion,
  point counts, atlas dimensions, codebooks, quaternion packing modes and SH
  label bounds are validated before the upstream decoder runs. Its intermediate
  Gaussian arrays store some attributes as float16; tests allow the resulting
  quantization error while verifying all SH bands and color/opacity mappings.
- PLY property validation and degree-dependent SH channel mapping surround the
  upstream reader. Its 0.6 fast path checks a zipped property prefix without
  checking the full property count, which can misread lower-degree binary PLY.
  The adapter uses its general reader to avoid that defect.
- Scene validation rejects empty scenes, non-finite attributes and degenerate
  scales/quaternions. GPU buffers and viewport sizes are checked against device
  limits.
- One process-wide scene GPU device is reused. Orbit/pan/zoom views render to
  offscreen RGBA textures and read back on the calling worker.
- `bake_orbit` produces exactly 30 views over a full revolution, with no
  duplicated endpoint, checking cancellation between submissions. A loop has a
  128 MiB pixel budget and retains only CPU frames. `frame_at` performs CPU
  indexing over a 12-second cycle; the renderer can be dropped before playback.
- Tests exercise PLY/SPZ roundtrips, SPLAT record layout and malformed data,
  lower-degree binary PLY records and SH mapping, CPU playback wraparound, and
  native GPU rendering. The GPU test checks visible non-black pixels, changed
  pixels for orbit/pan/zoom, 30 distinct view slots, playback after dropping the
  renderer, and cancellation.

Validated commands:

```sh
cargo test -p little-clip-scene -- --include-ignored
cargo clippy -p little-clip-scene --all-targets -- -D warnings
cargo fmt --all --check
```

All 17 scene tests passed, including native GPU rendering and SOG/GLB-to-orbit
bakes. GLB tests cover normalized attributes, SH degrees 0–3, multiple node
instances, nonuniform scale, directional color, invalid spans/hierarchies, and
both color spaces. Compressed PLY and SPZ v4 use real exporter fixtures and reject truncation. SPZ
checks cover oversized counts, invalid headers, corrupt gzip checksums,
v4 stream tables and exact parity with v3 packed directional color. SOG tests cover v1/v2 with SH degrees 0–3 and malformed data. These use
synthetic fixtures, **not** evidence of real-scene quality, large
scene performance, or completed application behavior.

The display/mirror cache uses file path, modification time, file size, viewport
size and appearance as its identity. It admits work through `BackgroundJobs`,
bakes 30 frames, drops the scene renderer, and retains CPU frames in an eight-entry,
128 MiB LRU cache. Source and appearance changes cancel superseded jobs. A native
GPU cache test proves 100 playback requests share one baked loop with one bake.

The workbench keeps one outstanding render, reuses its last accepted texture,
and presents completed camera views in order while coalescing newer navigation
into the next render. Exact camera equality would discard frames throughout a
continuous drag; only replaced viewport dimensions invalidate an in-session raster.
Closing/replacing the session cancels its job and drops its completion channel.
The GPU navigation test verifies replaced-viewport rejection, retained texture identity,
and no new jobs over 100 unchanged-view requests. A two-Gaussian SOG regression
test checks that every completed camera frame is presented even when more drag
input arrives before completion, then verifies convergence when input stops. Routing and startup queue tests
also pass. The production clipboard text-path card test verifies animation
classification, background cache admission and visible splat pixels in the
resulting display raster. Left drag orbits, right/middle drag pans, wheel zooms, and Reset view
restores framing. Rendering and imports run off the UI thread.

Private Weston/Niri acceptance with a synthetic 100-Gaussian SPLAT scene passed:
visible scene, centered fixed-size popup from mapping, stable min/max size hints,
single-instance focus and Escape returning to the workbench. Separate Light,
private orbit-drag and wheel-zoom runs also passed; screenshots show changed
scene orientation and increased zoom. The runner now
accepts `--scene`. These fixtures prove application behavior, not real-world
scene quality or large-scene performance.

Architecture baseline: `.fitness/baselines/pre-splat-viewer.json`. Shared card
routing removes the duplicated mesh/splat dispatch between URI and text cards.
Metadata entry validation is separated from ZIP traversal. No new Rust function exceeds
the complexity threshold. The acceptance CLI adds one branch to its existing
argument handling. Remaining clone reports include standard wgpu bootstrap
also needed independently by Film Look, test fixtures and existing UI boilerplate;
new container byte-read duplication was removed with one bounded reader.
The remaining bootstrap and independent test-fixture duplication is intentional.
The comparison reports REGRESSION: clone groups increased by two, while duplication
fell from 8.80% to 8.62%, maximum complexity stayed at 196, and dependency cycles
stayed at two. The new format adapters add necessary domain logic; duplicated
rendering and bounded-read mechanisms were consolidated. Strict Clippy passes for the scene crate.
Broader strict Clippy is blocked by existing warnings in core/render and vendored
code; no unrelated lint cleanup is included. `cargo test --workspace --
--test-threads=1` passed. The default parallel workspace run failed with shared
job-queue saturation and shared temporary fixture collisions; serial execution
removed those failures. `cargo fmt --all --check` and `git diff --check` pass.

## Real-scene validation

The [Spark butterfly sample](https://sparkjs.dev/assets/splats/butterfly.spz)
contains 177,132 Gaussians. PlayCanvas SplatTransform 3.9.0 converted it to PLY,
SPLAT, Gaussian GLB, compressed PLY, SPZ v4 and bundled SOG in `/tmp/lsc-real-scene`. Exported orbit
frames were visually inspected and show the butterfly. Format conversions can
change coordinate conventions (the GLB exporter uses glTF space). Private Niri
workbench captures visibly verified every requested format and the ordinary mesh
GLB fallback in Light/Dark runs. A real SPZ v4 capture also verified wheel zoom.
With `WGPU_BACKEND=none`, the viewer shows a readable unavailable-adapter error
and still returns with Escape. A temporary acceptance harness rendered real
clipboard-path cards for all five formats in both appearances; viewport pixels
and PNGs confirmed visible scenes. An earlier 4-second debug-build capture showed only
the loading state; a 15-second settled capture established the visible result.

Optimized runs with `cargo run --release -p little-clip-scene --example
scene_orbit -- <path> <output-directory>` produced these observed timings at
480×320, including 30-frame readback:

| Container | Load | GPU initialization/upload | Bake 30 | Total |
| --- | ---: | ---: | ---: | ---: |
| SPZ | 34 ms | 610 ms | 132 ms | 775 ms |
| PLY | 319 ms | 229 ms | 116 ms | 665 ms |
| SPLAT | 15 ms | 225 ms | 119 ms | 359 ms |
| Gaussian GLB | 189 ms | 224 ms | 121 ms | 534 ms |
| Compressed PLY | 76 ms | 315 ms | 134 ms | 525 ms |
| SPZ v4 | 43 ms | 450 ms | 148 ms | 640 ms |
| SOG v2, SH0 | 48 ms | 396 ms | 120 ms | 565 ms |
| SOG v2, SH3 (2,000 splats) | 3 ms | 335 ms | 121 ms | 459 ms |

These are individual measurements on the development machine, not portable
performance promises. Each process initializes its GPU. 10,000 CPU frame-index
lookups after dropping the renderer took 31–51 microseconds. Cache tests separately
prove repeated display requests reuse the same loop. The diagnostic example saves
frames 0 and 15 for visual inspection. SOG validation includes a full butterfly
scene with higher SH bands removed and a reduced 2,000-splat export preserving
all three SH bands. Two expensive full-scene CPU SH-clustering conversions were
cancelled after these independent exported variants passed.

Final validation passed:

```sh
cargo build --release --workspace
cargo test --workspace -- --test-threads=1
cargo test -p little-clip-scene -p little-clip-render -- --include-ignored --test-threads=1
cargo test -p clip-workbench splat -- --include-ignored --test-threads=1
cargo clippy -p little-clip-scene --all-targets --no-deps -- -D warnings
cargo fmt --all --check
git diff --check
```

The affected suites contain 17 scene tests, 24 render tests and five targeted
workbench tests, including native GPU checks. Physical USB hardware was not
exercised; display acceptance used the production card renderer and private
compositor captures.

## Supported variants and limits

- Gaussian PLY: standard 3DGS float attributes and PlayCanvas compressed PLY.
  Ordinary mesh/point-cloud PLY produces an explicit unsupported-data error.
- SPLAT: established 32-byte records (SH0).
- SOG: bundled v1/v2, SH degrees 0–3. Streamed LOD/meta.json input is not included.
- GLB: uncompressed `KHR_gaussian_splatting` with SH degrees 0–3, or ordinary
  meshes through the existing mesh viewer. Compressed Gaussian extensions,
  external buffers, mixed mesh/splat primitives and unsupported transforms
  produce explicit errors.
- SPZ: v1–v4, SH degrees 0–3. Vendor extensions, including alternative coordinate
  systems, are rejected rather than silently rendered with wrong orientation.
- Imports are bounded to two million Gaussians. Container byte budgets are
  512 MiB (SPLAT is limited by its 64 MB record budget); SOG atlases and expanded
  ZIP entries have additional limits. The display cache retains at most 128 MiB
  of CPU orbit pixels across eight entries.

Manual regression samples are in `/home/vega/Documents/type_testing/`: three files
per requested extension, with provenance, coverage and launch instructions in
`README-3D-viewer.md`. Those personal test assets are outside the repository.
