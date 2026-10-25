# Photo similarity review tasks

Review of `feat/photo-similarity-canvas` at `2dcc2b1`, compared with local main (`d52e624`).

## Current priorities

- [x] Verify actual SigLIP GPU execution; configure acceleration and report fallback failures clearly. The opt-in vision test recorded 2,561 CUDA kernels on the RTX 3090. Both encoders now request CUDA, with explicit CPU fallback warnings.
- [x] Persist image embeddings in SQLite across file-manager restarts, validating source and model revisions before reuse. The photo cache is a separate database; fully cached indexing skips model initialization.

## Correctness and responsiveness

- [x] Retain the shared browser focus target in the similarity canvas; tile clicks now keep keyboard and clipboard shortcuts active across frames. The regression test fails without registration.
- [x] **P1:** Make recursive selection ranges and keyboard navigation use the displayed collection, rather than root-folder entries (`selection.rs`, gallery row actions).
- [x] **P1:** Remove synchronous, unguarded hover image probing; use guarded metadata workers. Replacing a hovered file with a FIFO can block the UI (`ui/gallery.rs`, `shelf/preview.rs`).
- [x] **P1:** Cancel or reject searches from an earlier dataset generation when folders or recursive scan results change (`similarity/canvas.rs`).
- [x] Resume interrupted scans and clustering when returning to Photos; retiring jobs currently leaves an unchanged sync signature that prevents restarting them.
- [x] Keep reverse-image queries and candidates in the same feature space. Avoid downgrading an entire neural batch to dHash when one image fails decoding.
- [x] Move PCA/grid layout and repeated similarity scoring off the UI thread; cache scores. A 5,000-image, 1,536-dimensional PCA took roughly 500 ms in an optimized isolated check.
- [x] Handle background queue admission, timeout, disconnection and search errors explicitly; clear busy state and expose actionable failures.
- [x] Include hidden-file policy and explicit refresh/deep-directory changes in recursive scan invalidation.
- [x] Include valid negative cosine similarities in inverse filters; distinguish missing features from negative scores.
- [x] Bound the in-memory feature cache and calculate progress from the current collection, rather than all previously visited folders.
- [x] Make grid-header painting and hit testing share geometry, including zoom and stacked-grid spacing.
- [x] Populate EXIF hover information through the metadata pipeline; current hover previews never populate the photo metadata field.
- [x] Include source revision/identity in hover texture cache keys so replacing a file does not retain its old preview.
- [x] Limit similarity collections to their owning folder/view so ordinary image viewing does not inherit stale navigation entries.

## Style, architecture and validation

- [x] Reuse the shared canvas edge-fade painter instead of duplicating it in the gallery.
- [x] Simplify gallery interaction/HUD code. Compact grid headers share typed actions and icon controls; thumbnail admission has one priority scheduler. Complexity fell from 71 to 9 for `similarity_gallery_ui`, 42 to 19 for the masonry gallery, and 23 to 18 for the toolbar.
- [x] Replace the camera emoji in application chrome with the shared camera SVG icon.
- [x] Make the multimodal alignment test explicitly opt-in and fail on initialization or fixture errors when enabled, rather than silently passing without exercising inference.
- [x] Complete broader validation. The final workspace run passed 2,637 tests with 33 opt-in/hardware tests ignored; formatting passed. The private Weston/Niri smoke test passed indexing, search, zoomed controls, recursive Ctrl+A selection and folder changes.

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

The earlier GPU/cache checkpoint passed 2,624 workspace tests (32 opt-in/hardware tests ignored), with no failures. The CUDA probes passed all four tests; a separate child-process probe also successfully ran depth inference with all CUDA devices hidden. Python SAM/LaMa helpers already prefer CUDA when their separate Python runtime provides it; the native sessions now share that policy explicitly.

Architecture checks against the pre-change baseline show fewer clone groups (1,447 → 1,446), lower average complexity and no new cycles. The indexing scheduler's complexity fell from 17 to 6. The checker still reports existing clone ranges in touched files as regressions after line shifts; those fragments were already present in the baseline. The additional storage, revision validation and GPU probes implement the requested behavior rather than duplicate the audio database.

Worker layout snapshots now share entry arrays and dense embedding buffers. PCA checks cancellation between iterations, packing and score generation run on bounded workers, and stale layout requests cannot replace newer grid state. Search allowance buttons use binary searches over cached sorted scores rather than recomputing dot products each frame.

## Classification batching

SigLIP vision inference processes at most 32 uncached images per call. The final partial batch can be smaller. Persistent hits are published without inference, and a failed batch is retried image by image to isolate bad sources. The vision encoder is the FP16 ONNX export with its external weights cached alongside it; both files contribute to the embedding-cache revision. The RTX 3090 benchmark selected 32. CPU-specific tuning has not been measured, and CPU fallback uses the same batch size. Image Studio tools operate on one chosen image per user operation.

## Review completion and validation

Each finding was addressed in its own commit. Final checks:

```bash
cargo test --workspace -- --test-threads=1
cargo fmt --all --check
python -m py_compile scripts/file_manager_photo_similarity_acceptance.py
python scripts/file_manager_photo_similarity_acceptance.py --appearance dark --output-dir /tmp/photo-similarity-acceptance
```

The workspace passed 2,637 tests with no failures and 33 opt-in/hardware tests ignored. The new focus regression was confirmed failing before the fix and passing afterward. The UI smoke test uses the private Weston/Niri runner, requires the FP16 SigLIP vision graph and weights plus the text model and tokenizer, and isolates configuration and the SQLite database. It verified 24 persisted neural embeddings, a red-image query, compact controls at 97% zoom, Ctrl+A selecting recursive entries, an eight-image subfolder and return to the original collection. The standard English Tesseract language data must be installed or supplied through `TESSDATA_PREFIX`.

Against the pre-review baseline, clone groups fell from 1,446 to 1,442, average complexity fell from 4.07 to 4.05, and dependency cycles remained at six. The checker still labels the comparison REGRESSION because it flags existing clone ranges after edits and shared setup patterns in regression tests; the reported script-cycle changes reorder existing members of untouched scripts. The original canvas grid constructors and score tests already contained the corresponding repeated patterns. No new production complexity hotspot remains from these fixes. The added source implements cancellation, revision validation, worker scheduling and regression coverage; it does not duplicate the audio database.

### RTX 3090 batch-size benchmark

An opt-in raw inference benchmark compares the cached FP16 export against the cached INT8 (`vision_model_quantized.onnx`) export. Both use the same harness: a separate profiled CUDA verification session, two warmups, five timed inferences in an unprofiled session, and a fresh process per model/batch. The benchmark uses zero-filled tensors, so it excludes image decode and thumbnail preprocessing. A 100 ms `nvidia-smi` poll stopped a run below 12 GiB free. Other GPU work was active, so throughput and sampled free-memory minima are directional.

| Batch | FP16 images/sec | INT8 images/sec | FP16 sampled minimum free VRAM | INT8 sampled minimum free VRAM |
|---:|---:|---:|---:|---:|
| 1 | 70.18 | 2.03 | 14.9 GiB | 17.8 GiB |
| 8 | 83.70 | 2.01 | 14.9 GiB | 16.3 GiB |
| 16 | 87.52 | 1.99 | 14.9 GiB | 15.5 GiB |
| 32 | 90.08 | 2.47 | 14.9 GiB | 13.4 GiB |

The apples-to-apples run confirms FP16 is dramatically faster on this RTX 3090 (roughly 35–45× for these batches). The profiled INT8 run recorded 2,561 CUDA execution nodes and 415 CPU execution nodes, compared with 1,451 CUDA and 3 CPU nodes for FP16. This indicates substantial INT8 CPU execution/fallback and likely explains why its quantization does not translate into faster inference here. Batch 32 remains the FP16 default: it delivered the best throughput in the earlier sweep while retaining more than 14 GiB free in that run. INT8 memory use was lower for small batches, but its batch-32 run reached the 12 GiB safety boundary much more closely. Memory samples are affected by other GPU clients and should not be treated as exact per-model allocation.

The 2.33 GB external FP16 weight file is stored separately from the small ONNX graph.

The ignored benchmark can be repeated one size per process, up to 32:

```bash
SIGLIP_BENCH_MODEL=fp16 SIGLIP_BENCH_BATCH_SIZE=32 cargo test -p clip-workbench --lib benchmark_siglip_cuda_batch_size -- --ignored --nocapture
SIGLIP_BENCH_MODEL=int8 SIGLIP_BENCH_BATCH_SIZE=32 cargo test -p clip-workbench --lib benchmark_siglip_cuda_batch_size -- --ignored --nocapture
```
