# Terrain loading measurements and storage

Measured on the development desktop against its CIFS-mounted asset directory,
2026-11-18. These are source preparation measurements, not GPU frame times.
The opt-in test uses the real loaders, two workers and the viewer's source tile
selection at 1280×800. Its second pass deliberately regenerates source tiles;
a real viewer retains completed source tiles separately and normally skips this work.

## Findings and changes

The first preview used unoptimized workspace code. Cargo's
`profile.dev.package."*"` optimization does not cover workspace members.
Optimizing core reduced the medium Omaha view from 12.8 seconds to 1.4–1.6
seconds. Core now has an explicit development-profile override.

Inventory opened every one of the 14,520 degree tiles through the guarded
reader. Deferring guarded payload opening until actual use reduced inventory
from 13–17 seconds to about 0.9 seconds. Inventory rejects nonregular entries;
the actual read still validates the pinned file and its revision.

Sampling converted all 13 million heights in a tile before using four of them,
then took the global cache mutex and cloned its Arc for each pixel. It now
samples big-endian values directly, retains the current degree tile per raster
worker and selects source resolution once per output tile. The cache promotes
hits and trims geometric read-buffer overcapacity before byte accounting.
The medium view fell to 0.9–1.0 seconds, with 13–14 loads instead of 21–22
for 12 unique degree tiles. Close Omaha fell to about 0.42 seconds cold / 0.31
seconds cached. No SIMD-specific implementation was needed.

The shared HGT cache remains 128 MiB. Each of the two raster workers retains
at most one additional 26 MB degree tile if that entry is evicted from the
shared cache. The separate completed source-tile cache remains bounded.

## Compression comparison

Six original NASADEM ZIPs: `n41w096`, `n39w106`, `n27e086`, `n46e008`,
`n00w060`, `n30e031` (plains, mountains, Himalayas, Alps, rainforest, desert).
Every format's decoded bytes were checked against the existing downloaded HGT.
Sizes below are decimal MB. These samples do not establish total worldwide size.

| Format | Total stored MB | In-memory decode, approximate mean ms/tile |
| --- | ---: | ---: |
| Raw HGT | 155.61 | 0 |
| Original NASA ZIP | 76.54 | 44 |
| Original HGT member only (without quality/water entries) | 73.37 | 44 |
| Zstandard level 3 | 69.32 | 26 |
| Zstandard level 9 | 59.75 | 24 |
| OpenZL v0.3.0 `be-i16`, level 6 | 54.81 | 24 |

ZIP and Zstandard CPU measurements use in-memory Python standard ZIP/zlib and
python-zstandard decoders, five iterations per tile. OpenZL uses its own
in-memory `zli benchmark`, five iterations per tile; subprocess startup and
output-file writes are excluded from those decode figures. OpenZL numbers are
`dtimeMs / iters` in its CSV. No training or terrain-specific preprocessing was
used. Zstandard level 9 took roughly half a second per tile to encode on these
samples; level 3 was quicker but saved substantially less space.

A separate randomized NAS test, five rounds of all six tiles, evicted each
file's **client** page-cache pages with `POSIX_FADV_DONTNEED`. It did not flush
the server or claim physical-disk cold reads. Medians across 30 measurements:

| Format | Read ms | Decode ms | End-to-end ms |
| --- | ---: | ---: | ---: |
| Raw | 17.2 | 0 | 17.5 |
| Original ZIP | 8.4 | 43.8 | 52.8 |
| Zstandard 9 | 8.3 | 24.8 | 32.2 |

Compressed reads transfer less, but decompression makes total load time slightly
higher than raw on this connection. Inventory and CPU/cache work were the larger
problems. Zstandard 9 was selected for disk savings: about 62% versus raw and
22% versus original ZIP on these samples (19% versus just its HGT members),
with faster decoding than ZIP.
OpenZL saved another 8% versus Zstandard while decoding at effectively the same
speed; the existing bundled Zstandard dependency avoids adding a native runtime
for that modest additional saving. Terrain remains lossless; no pyramid or
additional on-disk resolution copies are generated.

## Storage and migration

New NASADEM and Viewfinder downloads stream their HGT member directly into a
checksummed, size-tagged `.hgt.zst` frame. ZIP CRC and exact expanded size are
validated before atomic installation. Runtime reads require a valid, bounded frame size and decode directly into that
one output allocation, using it for history instead of a separate streaming
window. Decoded bytes are retained in the bounded HGT cache.
Disk-budget planning uses conservative uncompressed sizes, and extraction charges
the actual installed byte count.

Legacy `.hgt` files remain supported. The standalone local migration helper does
not download anything:

```sh
cargo run -p little-clip-core --example compress_terrain -- /path/to/assets/nasadem/terrain
cargo run -p little-clip-core --example compress_terrain -- /path/to/assets/terrain
```

It runs four bounded workers, verifies each frame byte for byte, installs it
atomically, syncs the directory, then quarantines and checks the original's
identity before removal. Changed sources are retained. Interrupt with SIGINT or
SIGTERM and rerun the same command to resume. Mixed directories work, including
raw files converted after the viewer's initial inventory.

To repeat source preparation measurements with an installed dataset:

```sh
LITTLE_CLIP_GEOGRAPHY_ACCEPTANCE_DIR=/path/to/assets \
  cargo test -p little-clip-core --lib terrain_loading_profile \
  -- --ignored --nocapture --test-threads=1
```

Set `LITTLE_CLIP_GEOGRAPHY_PROFILE_CASE` to `omaha-close`, `omaha-medium`, or
`rockies-close` to measure one case. This test does not flush system caches.

## Architecture and verification

Compared with `a310c504`, duplication fell from 7.26% to 7.25%, clone groups
remained at 4,316, maximum complexity remained 274, and dependency-cycle count
remained 143. New functions stay at or below complexity 15. Existing ordinary
file snapshots, source identities, directory pins, atomic installation and the
bundled Zstandard dependency are reused. Native and byte-backed height sampling
share one interpolation implementation. The comparison tool still flags old
clones in the modified basemap file and differently ordered cycle descriptions;
those do not represent additional clone groups or dependency cycles.

`cargo test --workspace -- --test-threads=1` passes. The default parallel run
saturates shared image/audio studio test worker queues; its representative failed
image-diff test passes in isolation and all 843 studio tests pass serially.
The migration example additionally has focused interruption/resume and replaced
source retirement tests, and was exercised with six round-trip-verified files
on the actual CIFS mount before conversion of the installed dataset.

A final mixed raw/compressed Omaha check during the background bulk conversion
reported 1.46 s inventory, 2.44 s first preparation (1.27 s in guarded reads),
and 1.19 s second preparation (0.27 s in reads). HGT decoding took 0.10/0.07 s.
These are under concurrent NAS traffic, unlike the earlier quiet CPU comparison;
after removing the CPU costs, cold storage latency is a meaningful remaining
part of loading. Conversion is deliberately run at lower CPU priority.


The completed local NASADEM migration verified all **14,520** frames with no
errors and reduced actual terrain storage from **376.57 GB to 91.08 GB** (about
**76%** saved). No raw HGT files remain in either installed terrain directory.
This worldwide result is distinct from the six-sample format comparison above.
The final full-catalog plan recognizes every compressed tile without scheduling
a download. Post-migration regional source preparation measured **1.39 s / 1.24 s**,
with the complete 14,520-coordinate inventory loaded in **1.16 s**. The migration
race and requirement-by-requirement review are recorded in
[the renderer audit](map-rendering-review-audit.md).
