# Photo similarity review tasks

Review of `feat/photo-similarity-canvas` at `2dcc2b1`, compared with local main (`d52e624`).

## Current priorities

- [ ] Verify actual SigLIP GPU execution; configure acceleration and report fallback failures clearly.
- [ ] Persist image embeddings in SQLite across file-manager restarts, validating source and model revisions before reuse.

## Correctness and responsiveness

- [ ] **P1:** Make recursive selection ranges and keyboard navigation use the displayed collection, rather than root-folder entries (`selection.rs`, gallery row actions).
- [ ] **P1:** Remove synchronous, unguarded hover image probing; use guarded metadata workers. Replacing a hovered file with a FIFO can block the UI (`ui/gallery.rs`, `shelf/preview.rs`).
- [ ] **P1:** Cancel or reject searches from an earlier dataset generation when folders or recursive scan results change (`similarity/canvas.rs`).
- [ ] Resume interrupted scans and clustering when returning to Photos; retiring jobs currently leaves an unchanged sync signature that prevents restarting them.
- [ ] Keep reverse-image queries and candidates in the same feature space. Avoid downgrading an entire neural batch to dHash when one image fails decoding.
- [ ] Move PCA/grid layout and repeated similarity scoring off the UI thread; cache scores. A 5,000-image, 1,536-dimensional PCA took roughly 500 ms in an optimized isolated check.
- [ ] Handle background queue admission, timeout, disconnection and search errors explicitly; clear busy state and expose actionable failures.
- [ ] Include hidden-file policy and explicit refresh/deep-directory changes in recursive scan invalidation.
- [ ] Include valid negative cosine similarities in inverse filters; distinguish missing features from negative scores.
- [ ] Bound the in-memory feature cache and calculate progress from the current collection, rather than all previously visited folders.
- [ ] Make grid-header painting and hit testing share geometry, including zoom and stacked-grid spacing.
- [ ] Populate EXIF hover information through the metadata pipeline; current hover previews never populate the photo metadata field.
- [ ] Include source revision/identity in hover texture cache keys so replacing a file does not retain its old preview.
- [ ] Limit similarity collections to their owning folder/view so ordinary image viewing does not inherit stale navigation entries.

## Style, architecture and validation

- [ ] Reuse the shared canvas edge-fade painter instead of duplicating it in the gallery.
- [ ] Simplify gallery interaction/HUD code: `similarity_gallery_ui` has cyclomatic complexity 71; existing gallery rendering grew from 14 to 42, and toolbar logic from 8 to 23.
- [ ] Replace the camera emoji in application chrome with the shared camera SVG icon.
- [ ] Make the multimodal alignment test explicitly opt-in and fail on initialization or fixture errors when enabled, rather than silently passing without exercising inference.
- [ ] Complete broader tests when build resources permit; previous workspace validation ran out of disk space. The focused gallery run passed 55 tests and formatting checks passed.

Architecture comparison against local main: +4,575 non-comment lines, +122 functions, +34 clone groups; duplication 5.67% → 5.68%, cycles unchanged at six. These are review baselines, not targets to reduce by moving code without simplifying it.
