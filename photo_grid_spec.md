Implement a high-performance photo/gallery view for the file manager with an adaptive, tightly packed tiled layout. It must scale comfortably to folders containing 10,000+ images, support jumping/scrolling directly into the middle of large galleries, and avoid decoding images unnecessarily.

## Tile geometry

Use a half-square integer grid. Images may be displayed using these five tile shapes:

- Square: `2×2` — 1:1
- Landscape: `3×2` — 3:2
- Wide landscape: `4×2` — 2:1
- Portrait: `2×3` — 2:3
- Tall portrait: `2×4` — 1:2

For each image, choose whichever allowed shape minimizes cropping while still producing a good overall packing.

The common source ratios this should handle well are:

- 1:1
- 4:3 / 3:4 — typical phone cameras
- 3:2 / 2:3 — typical DSLR/mirrorless cameras
- 16:9 / 9:16

The 3:2 and 2:3 intermediate tiles are intentional. They allow 3:2 photos to remain uncropped while keeping crop loss low for 4:3 and 16:9 sources.

Use cover-style center cropping initially. Preserve the original image; cropping is presentation-only.

## Packing requirements

Do NOT implement this as simple masonry.

Use constrained row packing with limited lookahead / dynamic programming. Optimize crop while guaranteeing clean row boundaries.

All ordinary row widths must sum to a whole number of square units. Because dimensions use half-square units, whole-square boundaries occur at even X coordinates.

A `2×4` tall portrait may descend from one row into the next.

When a tile descends:

- Its left and right boundaries must lie on whole-square boundaries.
- The region before the descender must therefore have a whole-number width.
- The following row may pack images into the available region around/before the descender.
- Descenders participate in packing of the next row; they are not independent masonry columns.

Conceptually:

    ┌───────────────┬─────┐
    │               │     │
    │   row above   │  T  │
    ├───────┬───────┤  T  │
    │       │       │     │
    │ next  │ next  │     │
    └───────┴───────┴─────┘

## End-of-gallery behavior

Do not produce pointless descenders.

If a `2×4` tile would descend into the final row, but there would be no remaining images before/alongside the descender because the gallery/chunk has run out of items, choose an appropriate non-descending shape instead.

Also penalize mostly-empty final rows. It is acceptable to choose a slightly less crop-optimal shape for preceding images when that produces a substantially cleaner ending.

## Optimization

For source aspect ratio `s` and tile aspect ratio `t`:

    retained = min(s / t, t / s)
    crop_loss = 1 - retained

Minimize approximately:

    total crop loss
    + wasted-space penalty
    + incomplete/ugly-row penalty

subject to the hard packing constraints.

Crop loss is important, but a tiny improvement in crop should not justify a substantially worse layout.

Generate legal row candidates and use limited lookahead/DP to choose among them.

Do NOT treat this as arbitrary/general-purpose 2D bin packing. There are only five legal geometries, so exploit that heavily.

## Large-gallery architecture

The layout must NOT require globally solving every preceding image before displaying an arbitrary position in a large folder.

Divide the ordered gallery into independently packable chunks, initially targeting roughly 32–64 images per chunk. Make this size easy to tune after profiling.

For example:

    chunk 0: images 0–47
    chunk 1: images 48–95
    chunk 2: images 96–143
    ...

Each chunk should be independently layoutable and cacheable.

Prefer completely independent chunks if visually acceptable. If cross-chunk state is necessary for descenders, keep the boundary state extremely small and deterministic. Do not create a dependency chain that requires laying out all previous chunks before chunk N.

A chunk should contain approximately:

    first_item
    item_count
    compact aspect/dimension metadata
    placements
    row_count
    calculated height
    layout-generation/version information

The gallery should maintain a lightweight chunk-height index so it can map scroll positions to approximately relevant chunks without instantiating every tile.

## Unknown chunk heights / scrollbar virtualization

Before a chunk has been packed, its exact height may be unknown.

Use an estimated chunk height based on the average height of known chunks. This allows the scrollbar to represent the entire gallery immediately.

As chunks are packed, replace estimated heights with exact heights and update the cumulative-height index.

Avoid visually disruptive scroll jumps when estimates are corrected. Preserve the currently visible content/anchor position when recalculating cumulative heights.

Once cheap aspect metadata is available for all files, it is acceptable to asynchronously compute every chunk layout in the background. Packing thousands of tiny aspect records should be very cheap compared with image decoding.

## Image metadata: do not decode images just to lay them out

Packing requires only:

    width
    height
    EXIF/orientation where applicable

Do NOT fully decode an image merely to discover its aspect ratio.

Implement cheap header/metadata probing for supported formats. For common formats, dimensions can generally be obtained from a tiny portion of the file:

- PNG: dimensions are in the IHDR near the beginning.
- GIF: dimensions are in the header.
- BMP: dimensions are in the header.
- JPEG: scan markers until the relevant SOF marker; do not decode compressed pixel data.
- WebP: obtain dimensions from VP8/VP8L/VP8X metadata.
- AVIF/HEIF: parse enough of the container metadata to obtain dimensions without decoding the image.
- RAW formats: prefer metadata and/or embedded-preview information rather than developing the RAW image.

Use an existing reliable Rust crate where appropriate rather than hand-writing fragile parsers for every format.

Cache dimensions, orientation, and derived aspect class so subsequent gallery opens usually require no image reads at all.

Quantizing an image into a preferred aspect/tile class is fine for layout purposes; retain exact dimensions/aspect ratio for scoring when useful.

## Lazy thumbnails

Separate these stages completely:

    directory enumeration
        ↓
    dimension/orientation metadata
        ↓
    layout
        ↓
    thumbnail decode
        ↓
    rendering

A file does NOT need a decoded thumbnail in order to participate in layout.

The gallery should therefore become structurally usable almost immediately, potentially showing placeholders in correctly sized/positioned tiles while thumbnails are still loading.

Only request thumbnail generation/decode for:

- currently visible images
- images slightly above/below the viewport
- optionally a small predicted scroll-ahead region

Do not decode thumbnails for thousands of offscreen files just because the folder was opened.

If an image contains a suitable embedded thumbnail/preview, especially RAW or HEIF-style formats, prefer using that for gallery display instead of decoding the full-resolution source.

Decode/resize from the smallest source representation that can satisfy the actual on-screen tile resolution at the current DPI/zoom level.

Cache generated thumbnails independently from layout metadata.

## Parallelization

Design the pipeline explicitly for parallel execution.

### Metadata probing

Once directory enumeration produces the file list, dimension/header probing for different files is independent and can run concurrently.

Use a bounded worker pool / Rayon-style parallelism rather than spawning one task/thread per file.

Do not let 10,000 images generate 10,000 simultaneous filesystem reads. Bound I/O concurrency to avoid destroying performance on HDDs, network mounts, or cold caches.

Consider storage characteristics: highly parallel metadata reads may help on SSD/NVMe but excessive random reads can hurt HDD performance. Keep concurrency configurable or adaptive.

### Layout

Independent gallery chunks should be layoutable in parallel.

Prioritize:

1. visible chunk(s)
2. immediately adjacent chunks
3. chunks in the current scroll direction
4. remaining chunks as low-priority background work

Once aspect metadata exists, multiple chunks can be packed simultaneously.

The UI/render thread must never block waiting for background layout of unrelated chunks.

### Thumbnail generation

Thumbnail decoding/resizing is also parallelizable, but use a separate bounded work queue because decoding is CPU/memory intensive.

Prioritize thumbnails by distance from the viewport.

If the user rapidly scrolls elsewhere, stale low-priority thumbnail jobs should be cancellable or deprioritized rather than wasting substantial work on images that are no longer nearby.

Avoid multiple simultaneous decodes of the same source image.

The rough architecture should therefore resemble:

    directory enumeration
          │
          ├── metadata/header worker pool
          │         │
          │         └── aspect metadata cache
          │                    │
          │                    ├── parallel chunk layout
          │                    │
          │                    └── viewport/virtualization
          │                               │
          │                               └── prioritized thumbnail queue
          │
          └── UI remains responsive throughout

## Caching

Maintain separate caches for:

1. File metadata
   - dimensions
   - orientation
   - aspect ratio/class
   - modification time / identity information needed for invalidation

2. Gallery layout
   - viewport/grid configuration
   - chunk placements
   - chunk heights

3. Thumbnails
   - decoded/resized display representation
   - requested resolution/scale as appropriate

Changing gallery width should invalidate/recompute layout without invalidating image metadata or thumbnails unnecessarily.

Changing/modifying one image should invalidate only the affected metadata, thumbnail, and relevant layout chunk(s), not the entire gallery.

## Resizing

The number of horizontal grid units should depend on available gallery width.

Recompute packing when the viewport changes enough to alter the useful grid configuration. Do not continuously repack everything because of insignificant sub-pixel resize changes.

Prioritize recomputation of visible chunks after resize. Offscreen chunks can be recomputed lazily/background.

Keep layout generation completely separate from rendering.

## Virtualization

Do not create UI widgets/resources for all 10,000+ gallery items.

Determine which chunks intersect the viewport plus overscan, and instantiate/render only those items.

Scrolling to the middle of a huge folder should therefore require roughly:

- finding the relevant chunk using the height index
- ensuring that chunk's metadata/layout exists
- rendering placeholders immediately if necessary
- asynchronously loading nearby thumbnails

It should NOT require decoding or rendering all preceding files.

## Testing

Add tests for:

- all-square images
- all 4:3 images
- all 3:2 images
- all 16:9 images
- mixed portrait/landscape sets
- many tall portrait images
- tall portrait near the end
- tall portrait as the final image
- descender fractional-width violations
- cases where greedy packing is visibly worse than lookahead
- galleries containing 1, 2, 3, 4, and 5 images
- chunk-boundary behavior
- resizing between gallery widths
- jumping directly into the middle of a 10,000-image synthetic gallery
- unknown → known chunk-height correction without visible scroll jumps
- metadata arriving out of order due to parallel workers
- thumbnail jobs completing out of order
- rapid scrolling causing thumbnail priorities to change
- file modification correctly invalidating caches

Assert these invariants:

- tiles never overlap
- tiles never exceed gallery width
- required boundaries occur on whole-square coordinates
- regions before descenders have whole-number widths
- no pointless final descender exists
- every input image appears exactly once
- chunking does not reorder files
- layout does not depend on thumbnail availability
- UI operations do not synchronously decode offscreen images

## Performance goal

Opening a huge image directory should feel progressive rather than blocking.

The desired behavior is:

1. Enumerate filenames quickly.
2. Show the gallery structure/placeholders as metadata becomes available.
3. Prioritize metadata necessary for the visible region.
4. Produce visible chunk layouts immediately.
5. Decode visible thumbnails in parallel.
6. Prefetch nearby thumbnails.
7. Fill in metadata/layout for the rest of the gallery opportunistically in the background.

Do not make "gallery ready" synonymous with "all images decoded."

Architect the metadata probing, packing, virtualization, and thumbnail systems as separate components with clear interfaces so each can be profiled and optimized independently.