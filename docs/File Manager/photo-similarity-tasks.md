# Photo similarity review tasks

Review of `feat/photo-similarity-canvas` at `2dcc2b1`, compared with local main (`d52e624`).

## Current priorities

- [x] Verify actual SigLIP GPU execution; configure acceleration and report fallback failures clearly. The opt-in vision test recorded 2,561 CUDA kernels on the RTX 3090. Both encoders now request CUDA, with explicit CPU fallback warnings.
- [x] Persist image embeddings in SQLite across file-manager restarts, validating source and model revisions before reuse. The photo cache is a separate database; fully cached indexing skips model initialization.

## Correctness and responsiveness

- [x] **P1:** Make recursive selection ranges and keyboard navigation use the displayed collection, rather than root-folder entries (`selection.rs`, gallery row actions).
- [x] **P1:** Remove synchronous, unguarded hover image probing; use guarded metadata workers. Replacing a hovered file with a FIFO can block the UI (`ui/gallery.rs`, `shelf/preview.rs`).
- [x] **P1:** Cancel or reject searches from an earlier dataset generation when folders or recursive scan results change (`similarity/canvas.rs`).
- [x] Resume interrupted scans and clustering when returning to Photos; retiring jobs currently leaves an unchanged sync signature that prevents restarting them.
- [x] Keep reverse-image queries and candidates in the same feature space. Avoid downgrading an entire neural batch to dHash when one image fails decoding.
- [ ] Move PCA/grid layout and repeated similarity scoring off the UI thread; cache scores. A 5,000-image, 1,536-dimensional PCA took roughly 500 ms in an optimized isolated check.
- [x] Handle background queue admission, timeout, disconnection and search errors explicitly; clear busy state and expose actionable failures.
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

## GPU and cache behavior

All native ORT image models use the shared model loader and request CUDA device 0. CUDA registration or session-initialization failure emits a warning and retries with fresh CPU session options. The runtime trace test verifies actual vision kernel execution, rather than merely enumerating available providers:

```bash
cargo test -p clip-workbench --lib vision_kernels_execute_on_cuda -- --ignored --nocapture
```

This opt-in test requires the cached vision model and a working NVIDIA CUDA/cuDNN installation. The new cache tests do not require a GPU.

Photo features live in `$XDG_CACHE_HOME/little-screen-clip/photo-similarity.sqlite` (default `~/.cache/little-screen-clip/photo-similarity.sqlite`). This database has its own connection, WAL and schema, independent of `constellation.sqlite`. Cache failures warn and fall back to indexing without persistence.

Rows use raw Unix path bytes, source device/inode/size/mtime/ctime, and an encoder key including preprocessing version and model identity. Source replacement, image edits and model replacement invalidate reuse. Sources are revalidated before publication and storage; model changes during loading/inference reject the affected neural result. Cached neural vectors must have the expected dimensions, finite values and unit norm. Perceptual hashes use a separate encoder key and are upgraded when the neural model is available.

SQLite reads and writes run on indexing workers. Fully cached batches publish without constructing an inference session; partially cached batches compute only missing features. Tests cover reopening the database, skipping initialization on restart, cancellation, source edits/replacement, model-key changes, invalid vectors and byte-exact filenames.

## Shared CUDA verification

The opt-in model probes use the same model loader as the file manager and Image Studio. All seven cached models executed CUDA kernels on the RTX 3090:

| Model | CUDA kernels in probe |
| --- | ---: |
| SigLIP vision | 2,561 |
| SigLIP text | 1,716 |
| FeyNoBg background removal | 8,178 |
| Depth Anything V2 | 464 |
| LaMa inpainting | 1,773 |
| MobileSAM encoder | 475 |
| MobileSAM decoder | 364 |

```bash
cargo test -p clip-workbench --lib cuda -- --ignored --nocapture --test-threads=1
cargo test -p clip-workbench --lib cpu_fallback_without_visible_gpu -- --ignored --nocapture
cargo test --workspace -- --test-threads=1
```

Final workspace validation passed 2,624 tests (32 opt-in/hardware tests ignored), with no failures. The CUDA probes passed all four tests; a separate child-process probe also successfully ran depth inference with all CUDA devices hidden. Python SAM/LaMa helpers already prefer CUDA when their separate Python runtime provides it; the native sessions now share that policy explicitly.

Architecture checks against the pre-change baseline show fewer clone groups (1,447 → 1,446), lower average complexity and no new cycles. The indexing scheduler's complexity fell from 17 to 6. The checker still reports existing clone ranges in touched files as regressions after line shifts; those fragments were already present in the baseline. The additional storage, revision validation and GPU probes implement the requested behavior rather than duplicate the audio database.
