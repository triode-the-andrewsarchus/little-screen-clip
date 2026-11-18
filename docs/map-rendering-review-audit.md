# GPU map renderer and original review audit

Audit of `feat/map-rendering` on 2026-11-18. The original review was against
`7eefae74`; the implementation inspected here includes fixes through `8a551252`.
This records the original findings independently of subsequent feature requests.
Personal runtime configuration and Cargo build-job overrides remain uncommitted.
The `feat/portable-drive-services` worktree was not modified.

## Renderer acceptance

The viewer calls `geography::tiles::Tiles::update`, which admits source preparation
on a shared two-worker executor and paints through the host's OpenGL callback.
CPU workers produce camera-independent color/elevation/water source tiles. The
production shaders perform projection, shading, lighting, contours and vector
projection. The old per-camera CPU rasterizer has been removed. This still needs
CPU file decoding/resampling; it does not claim that disk loading is GPU work.

The current production shaders compile, link and draw seven distinct raster
scenarios and both vector instance types on NVIDIA RTX 3090 and Mesa llvmpipe.
The native Rust renderer test also passes on both implementations: it checks
actual readback, source-texture reuse over camera changes, vector detail
identities, replacement asset generations, natural close-up colors, pure-black
Oilslick water, repeated flat-map edges and explicit GPU-resource destruction.
These are separate private EGL contexts, not desktop/compositor configuration
changes. No direct Intel hardware measurement was available.

## Original 13 findings

All rows below were checked against the current implementation and regression
coverage, rather than inferred solely from commit subjects. Source paths are
relative to the repository root. Test names identify assertions inspected during
this audit; the commands below rerun their containing suites.

| # | Original problem | Current implementation and proving coverage | Fix commit |
| --- | --- | --- | --- |
| 1 | Expanded ZIP entries exhaust RAM/disk and ignore cancellation | `geography_downloader.rs::write_archive_payload` streams through 64 KiB buffers; extraction checks entry count, declared expansion, actual output bytes and cancellation before installation. HGT encoding separately bounds input size. `extraction_retains_its_directory_and_rejects_symlink_destinations` exercises cancellation and expansion-budget rejection without publication; `terrain_extraction_stores_a_lossless_compressed_frame` covers the newer compressed path. | `90e576ab`, `a777c61d` |
| 2 | Asset reads/writes bypass filesystem protections and lose directory ownership | Core `geography/basemap/storage.rs::AssetDirectory` retains an ordinary directory, guards source opens and atomically installs owned staging files. Tests reject symlink destinations, devices and FIFOs, retain the original directory across a parent rename and preserve previous output on failure. Extractor tests repeat the held-directory and symlink cases through ZIP installation. | `0548a008` |
| 3 | Missing overviews decode full global DEMs; mask cache has no byte budget | `load_global_dem` selects an optional bounded overview and returns `None` if absent. `CogDemPyramid` then samples bounded TIFF chunks; masks also decode requested chunks with byte accounting. `plain_dem_without_an_overview_is_sampled_by_chunks` proves the overview is refused, sampling still works and cache bytes stay bounded. `masks_decode_only_requested_strips_and_index_coverage` proves only the requested strips are retained. | `88d9d14e` |
| 4 | Detached jobs outlive viewers and accepted downloads lose their owners | Assets and raster/vector preparation use shared bounded executors and core `jobs::task::Task`; dropping read tasks requests cancellation while executor admission remains owned through actual worker completion. Downloads use retained unbounded-lifetime tasks, and geography's `writes_pending` participates in viewer/application close exclusion. `cancelling_an_accepted_download_retains_its_owner_until_worker_exit` proves cancellation does not release the accepted owner early; core Task deadline/completion tests pass. | `c6696ebf`, `9d45740a` |
| 5 | Every repaint walks/projects global vector datasets | `geography/geometry.rs` prepares bounded spatial batches on workers; `geography/tiles.rs` retains prepared scenes/river tiles and the GPU caches uploaded batches. Visibility/detail keys select regional work. `river_detail_keys_follow_visibility_and_reuse_small_camera_moves` and the native renderer's unchanged vector-buffer assertion verify reuse; projection is in the vector shader. | `9d45740a`, `5551907b` |
| 6 | Forward and inverse blended projections disagree | Core `GlobeCamera::unproject` inverts the actual blended forward projection with bounded damped iteration, retaining analytic pure-mode paths. `blended_projection_roundtrips_visible_points_across_the_transition` checks multiple centers, dateline positions and zooms including 3.15, with less than 0.02 pixel roundtrip error. Invalid/background pixels are separately rejected. Raster and vector shaders use the same forward equations. | `21517fd7` |
| 7 | Camera-quantized raster results remain stale; old work can replace current output | Source tiles no longer contain a camera snapshot. Camera uniforms update at drawing time. Replacing the asset Arc resets the viewer-owned tile set and assigns a new source generation; GPU keys include that generation. Task polling treats disconnected delivery as terminal, and tile-cache failures are retired on view changes. The native renderer checks reuse under sub-0.01-degree moves and different texture identities for replacement assets; core source-key tests prove camera reuse. | `9d45740a` |
| 8 | Downloaded NetCDF land cover is unusable and falsely installed | `geography/basemap/land_cover.rs::import_netcdf` converts `lccs_class` with nearest-neighbor GDAL resampling into a bounded validated categorical grid, then atomically installs it. Status requires the usable compact grid. The real NetCDF fixture test passes with isolated GDAL 3.13.3/NetCDF support and verifies north/south orientation, class mapping, cancellation preservation and budget rejection. GDAL remains an optional documented import dependency. | `5545f1d9`, `5b71c7f9` |
| 9 | Extraction failures appear successful and suppress retries | GEBCO and vector extraction propagate errors. Installed status comes from usable loaded data, not a directory or source archive alone. `installed_status_requires_usable_data_and_both_quick_start_layers` rejects empty HydroRIVERS directories and broken grids; `successful_transfer_of_unusable_data_is_reported` rejects unusable successful transfers. `failed_pack_keeps_and_publishes_layers_downloaded_before_and_after_it` proves independent successful layers remain available and failed layers are reported. | `dda065c7`, `af9b05fe` |
| 10 | Unicode filenames, malformed grids and corrupt vector caches panic | HGT names are validated as ASCII and checked before field parsing. Grid dimensions/lengths use checked limits. Both river cache readers bound counts and validate segment references before publication. `malformed_grids_and_unicode_tile_names_are_rejected`, `river_caches_reject_unbounded_counts_truncation_and_invalid_references` and `invalid_dem_cache_does_not_hide_a_valid_source_overview` exercise these cases. | `784961cc`, `e3dd2397` |
| 11 | Fit centers dateline tracks at Greenwich and hides them | Parsed documents retain compact wrapped geographic bounds from all source positions; viewer Fit uses those bounds. `fitting_dateline_documents_keeps_the_track_visible` parses +179/-179, checks the center and both projected endpoints, preserves forced globe mode and handles empty bounds. A separate local-walk fit test prevents impractically distant zooms. | `473ba726`, `bb55533a` |
| 12 | Convex lake fills erase concavity/islands | Lake parsing preserves interior rings; `geometry.rs::triangulate` uses hole-aware polygon triangulation for GPU triangle instances. `lake_parser_preserves_island_rings_for_gpu_tessellation` checks parsing; `lake_tessellation_preserves_concavity_and_islands` checks the area of a concave polygon minus its island. The native shader acceptance draws triangle instances successfully on both GL implementations. | `9d45740a` |
| 13 | Multipart rivers acquire imaginary connecting segments | Shared `shp_polyline_parts` validates the part table and supplies independent parts to both river loaders, retaining endpoints and per-feature metadata. Versioned GRWL/HydroRIVERS caches exclude legacy joined geometry. `both_river_loaders_preserve_multipart_boundaries_and_cache_them` checks both loaders, spatial queries and a cache-only reload after removing source SHPs; malformed part offsets are separately rejected. | `777990c7` |

## Verification performed

- Latest focused core geography suite: **47 passed**, two opt-in tests skipped.
- Latest focused file-manager geography/download suite: **30 passed**, two opt-in tests skipped.
- Core Task lifecycle suite: **4 passed**.
- Opt-in native Rust GPU renderer: **passed on NVIDIA and Mesa llvmpipe**.
- Production shader acceptance: **7 raster + 2 vector scenarios passed on each implementation**, with GL error checks.
- Opt-in real NetCDF/GDAL conversion: **passed** with the isolated NetCDF-capable toolchain.
- Workspace run recorded in the terrain performance report: **3,370 passed, 40 ignored**, using serial test execution. The default parallel studio run saturates shared test worker queues; serial studio coverage passes all 843 cases.
- Changed Rust files pass targeted rustfmt checks. Workspace-wide formatting includes unrelated vendored differences and is not rewritten.

Reproducible ordinary checks:

```sh
cargo test -p little-clip-core --lib geography -- --test-threads=1
cargo test -p clip-file-manager --lib file_manager::geography -- --test-threads=1
cargo test -p clip-file-manager --lib native_renderer_reuses_tiles_on_camera_changes_and_retires_resources -- --ignored --test-threads=1
python3 scripts/geography_gpu_acceptance.py --output /tmp/geography-gpu-acceptance
LIBGL_ALWAYS_SOFTWARE=1 __EGL_VENDOR_LIBRARY_FILENAMES=/usr/share/glvnd/egl_vendor.d/50_mesa.json \
  python3 scripts/geography_gpu_acceptance.py --output /tmp/geography-gpu-acceptance-mesa
cargo test --workspace -- --test-threads=1
```

Run the ignored `imports_real_netcdf_with_south_to_north_coordinates_and_preserves_categories`
test with GDAL 3.7+ and its NetCDF driver available on PATH to repeat import
acceptance. Toolchain installation is separate from the bundled renderer.

## Subsequent requests and remaining external work

Units, flat-map wrapping, restored high-resolution shading, pure-black Oilslick
water, regional Viewfinder downloads, authenticated/resumable NASADEM downloads,
river-source priority/fallback and bounded compressed terrain streaming are
implemented and covered by their scoped tests and the GPU checks above.
[Terrain loading measurements](map-terrain-performance.md) record the CPU/cache
and storage-format comparison and its limitations.

The complete 14,520-tile NASADEM download finished before local compression
started. New downloads already install `.hgt.zst` files. Conversion of the existing
raw files is an independent, still-running process: four bounded workers verify
each frame byte for byte before installing it and retiring its exact original.
Mixed raw/compressed inventories remain usable while it runs. Completion of that
local migration must be checked against its live process, final log, remaining
raw files and resulting inventory; it is not claimed complete by this audit.

## Concurrent migration follow-up

A fully compressed regional profile exposed a one-tile omission during directory
mutation: a single-pass enumeration reported 14,519 instead of 14,520 sources.
The loader now rechecks the existing directory metadata identity and merges one
additional scan only if it changed. A stale raw directory entry can use metadata
from its already-published compressed replacement. Payload reads similarly retry
the independently guarded compressed snapshot when the raw snapshot is retired
or otherwise unusable, before caching a failure. No directory-wide lock or
unbounded stabilization loop is introduced.

Deterministic tests cover a raw entry removed after enumeration and both guarded
read rejection and malformed raw content with a valid compressed replacement.
Three consecutive real-data profiles, while the converter remained active, each
asserted the full 14,520-source inventory. Their sampled Omaha region used only
compressed files. The opt-in profile can enforce an expected inventory with
`LITTLE_CLIP_GEOGRAPHY_PROFILE_EXPECT_HGT=14520`.
